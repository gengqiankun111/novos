//! M2 切片：内核线程 + 上下文切换 + 轮转调度 + 睡眠唤醒（tick 计数）。
//!
//! 设计要点：
//! - 上下文 = 中断帧（`ExceptionFrame`）：PIT tick 在任务内核栈上压入帧，
//!   调度器保存/切换 `ctx_rsp`（帧指针），irq_common 恢复后 iretq 继续运行；
//! - 固定任务表（MAX_TASKS），**不在 IRQ 上下文做堆分配**（防持分配器锁重入）；
//! - 任务 0 = idle（boot/main 上下文）；新线程栈经 mm 分配，4K 对齐；
//! - `sleep_ticks` 由任务自身调用：置 Sleeping + hlt 让出，tick 到期被唤醒。
//!
//! 对应 DEVELOPMENT.md M2；CFS vruntime 红黑树/RT 双队列留后续切片（M2 内迭代）。

// 任务表为 static mut，单核 IRQ 关闭访问；显式允许该 lint。
#![allow(static_mut_refs)]

use crate::interrupts::ExceptionFrame;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};

/// 内核线程栈大小（64KB，后续加 guard page）。
pub const STACK_SIZE: usize = 64 * 1024;
/// 任务表上限（固定数组，防 IRQ 上下文堆分配）。
pub const MAX_TASKS: usize = 16;

#[derive(Clone, Copy, PartialEq)]
pub enum TaskState {
    /// 就绪（可被调度）
    Ready,
    /// 运行中
    Running,
    /// 睡眠（sleep_until tick 到期前不可调度）
    Sleeping,
    /// 阻塞（等待锁/事件，由 sync 原语唤醒）
    Blocked,
    /// 已退出（waitpid 可回收，不再调度）
    Exited,
    /// 空闲（任务 0）
    Idle,
}

#[derive(Clone, Copy)]
pub struct Task {
    pub name: &'static str,
    pub state: TaskState,
    /// 保存/恢复的 rsp：指向该任务最近一次被中断的 ExceptionFrame。
    pub ctx_rsp: usize,
    /// 内核栈基址（后续回收用）。
    pub stack: *mut u8,
    /// Sleeping 唤醒时刻（tick 数）。
    pub sleep_until: u64,
    /// 基础优先级（越大越高；PIP 恢复目标）。
    pub base_prio: u8,
    /// 有效优先级（PIP 提升后的实际调度优先级）。
    pub effective_prio: u8,
    /// CFS vruntime（虚拟运行时间，红黑树键）。
    pub vruntime: u64,
    /// 累计被调度运行 tick 数（CFS 公平性观测）。
    pub run_ticks: u64,
    /// 是否在 runqueue 红黑树中。
    pub in_rq: bool,
}

impl Task {
    const fn empty() -> Self {
        Task {
            name: "",
            state: TaskState::Idle,
            ctx_rsp: 0,
            stack: ptr::null_mut(),
            sleep_until: 0,
            base_prio: 0,
            effective_prio: 0,
            vruntime: 0,
            run_ticks: 0,
            in_rq: false,
        }
    }
}

static mut TASKS: [Task; MAX_TASKS] = [Task::empty(); MAX_TASKS];
/// 已用任务数（0 号固定为 idle）。
static mut TASK_COUNT: usize = 1;
/// 当前运行任务下标（0 = idle）。
static mut CURRENT: usize = 0;
/// 全局 tick 计数（PIT 100Hz）。
///
/// 用 AtomicU64 而非 `static mut`：防止编译器在紧循环（如 worker-c 的忙等）
/// 中把 `static mut` 读取提升出循环，导致读到冻结的旧值。
static TICKS: AtomicU64 = AtomicU64::new(0);

/// 当前 tick 数。
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// 当前运行任务 id（0 = idle）。
pub fn current_id() -> usize {
    // SAFETY: 单核读。
    unsafe { CURRENT }
}

/// 任务基础优先级。
pub fn priority(id: usize) -> u8 {
    // SAFETY: 单核读。
    unsafe { TASKS[id].base_prio }
}

/// 任务有效优先级（PIP 提升后）。
pub fn effective(id: usize) -> u8 {
    // SAFETY: 单核读。
    unsafe { TASKS[id].effective_prio }
}

/// 任务累计运行 tick（CFS 公平性观测）。
pub fn run_ticks(id: usize) -> u64 {
    // SAFETY: 单核读。
    unsafe { TASKS[id].run_ticks }
}

/// 任务 vruntime（CFS 红黑树键）。
pub fn vruntime(id: usize) -> u64 {
    // SAFETY: 单核读。
    unsafe { TASKS[id].vruntime }
}

/// PIP：将任务 `id` 的有效优先级提升到 `prio`（只升不降）。
pub fn boost(id: usize, prio: u8) {
    // SAFETY: 单核，sync 在关中断区调用。
    unsafe {
        if TASKS[id].effective_prio < prio {
            TASKS[id].effective_prio = prio;
        }
    }
}

/// PIP 解除：恢复基础优先级。
pub fn restore_prio(id: usize) {
    // SAFETY: 单核，sync 在关中断区调用。
    unsafe {
        TASKS[id].effective_prio = TASKS[id].base_prio;
    }
}

/// 当前任务阻塞（等锁/事件），hlt 让出直到被 `wake`。
pub fn block_current() {
    // SAFETY: 任务上下文，单核。
    unsafe {
        crate::sync::cli();
        let cur = CURRENT;
        TASKS[cur].state = TaskState::Blocked;
        if TASKS[cur].in_rq {
            crate::smp::cpu_rq(0).rbt.remove(cur);
            TASKS[cur].in_rq = false;
        }
        crate::sync::sti();
        core::arch::asm!("hlt", options(nomem, nostack));
    }
}

/// 唤醒一个阻塞任务（由 sync 原语调用；**调用方须已关中断**——sync 的 unlock 在 cli 区）。
pub fn wake(id: usize) {
    // SAFETY: 单核写；cli 保护树操作。
    unsafe {
        if id != 0 && TASKS[id].state == TaskState::Blocked {
            TASKS[id].state = TaskState::Ready;
            if !TASKS[id].in_rq {
                crate::smp::cpu_rq(0).rbt.insert(id, TASKS[id].vruntime);
                TASKS[id].in_rq = true;
            }
        }
    }
}

/// 创建内核线程（栈经 mm 分配；任务函数必须永不返回，结束前自行停机/睡眠）。
pub fn spawn(name: &'static str, entry: fn(), prio: u8) -> Result<u32, &'static str> {
    // SAFETY: 创建线程仅在启动阶段（单核、无并发）调用。
    unsafe {
        if TASK_COUNT >= MAX_TASKS {
            return Err("task table full");
        }
        let idx = TASK_COUNT;
        TASK_COUNT += 1;

        let layout = core::alloc::Layout::from_size_align(STACK_SIZE, 4096).map_err(|_| "bad layout")?;
        let stack = alloc::alloc::alloc(layout);
        if stack.is_null() {
            return Err("stack alloc failed");
        }

        // 栈顶构造初始 ExceptionFrame（布局与 irq_common 一致）：
        // 首次被调度时 iretq → 跳 entry，rsp = 栈顶，IF=1。
        let frame_ptr = (stack as usize + STACK_SIZE - core::mem::size_of::<ExceptionFrame>()) & !15usize;
        let frame = &mut *(frame_ptr as *mut ExceptionFrame);
        *frame = core::mem::zeroed();
        frame.rip = entry as u64;
        frame.cs = 0x08; // boot GDT 的 code64
        frame.ss = 0x10; // data
        frame.rflags = 0x202; // IF=1（bit9）+ 保留位 bit1；0x2 只含保留位，会关中断
        frame.rsp = (stack as usize + STACK_SIZE) as u64;

        TASKS[idx] = Task {
            name,
            state: TaskState::Ready,
            ctx_rsp: frame_ptr,
            stack,
            sleep_until: 0,
            base_prio: prio,
            effective_prio: prio,
            vruntime: 0,
            run_ticks: 0,
            in_rq: false,
        };
        // 新任务入 CFS 就绪树（关中断防与 tick 调度竞争）
        // SAFETY: 单核；cli 保护树操作。
        unsafe {
            crate::sync::cli();
            crate::smp::cpu_rq(0).rbt.insert(idx, 0);
            TASKS[idx].in_rq = true;
            crate::sync::sti();
        }
        Ok(idx as u32)
    }
}

/// PIT tick（IRQ 上下文调用，IF 关闭）：CFS 记账 → 维护就绪树 → 取最小 vruntime 运行。
///
/// # Safety
/// 仅由 rust_irq_handler 调用，单核且中断关闭（树操作天然互斥）。
pub unsafe fn on_timer_tick(frame: *mut ExceptionFrame) -> *mut ExceptionFrame {
    let cur = CURRENT;
    TASKS[cur].ctx_rsp = frame as usize;
    if TASKS[cur].state == TaskState::Running {
        TASKS[cur].state = TaskState::Ready;
    }

    TICKS.fetch_add(1, Ordering::Relaxed);
    let now = TICKS.load(Ordering::Relaxed);

    // CFS 记账：当前任务 vruntime += 1024 / 权重（权重 = 1 << effective_prio）。
    // 权重高 → vruntime 增长慢 → 被选中频率高 → 获得更多 CPU（含 PIP 提升语义）。
    let rq = crate::smp::cpu_rq(0);
    if cur != 0 {
        let weight = 1u64 << TASKS[cur].effective_prio;
        TASKS[cur].vruntime += 1024 / weight;
        TASKS[cur].run_ticks += 1;
        // 树维护：先出树，若仍就绪再以新 vruntime 入树
        if TASKS[cur].in_rq {
            rq.rbt.remove(cur);
            TASKS[cur].in_rq = false;
        }
        if TASKS[cur].state == TaskState::Ready {
            rq.rbt.insert(cur, TASKS[cur].vruntime);
            TASKS[cur].in_rq = true;
        }
    }

    // 唤醒到期睡眠任务并入树（clamp 到最小 vruntime 防饿死）
    let min_vruntime = rq.rbt.min().map(|id| TASKS[id].vruntime);
    for i in 1..TASK_COUNT {
        if TASKS[i].state == TaskState::Sleeping && TASKS[i].sleep_until <= now {
            TASKS[i].state = TaskState::Ready;
            if let Some(m) = min_vruntime {
                if TASKS[i].vruntime < m {
                    TASKS[i].vruntime = m; // 唤醒 clamp：不落后于就绪队列
                }
            }
            if !TASKS[i].in_rq {
                rq.rbt.insert(i, TASKS[i].vruntime);
                TASKS[i].in_rq = true;
            }
        }
    }

    // 取最小 vruntime 就绪任务；树空 → idle
    let mut next = 0;
    if let Some(id) = rq.rbt.min() {
        if TASKS[id].state == TaskState::Ready {
            next = id;
        } else {
            // 树中有脏节点（状态非 Ready）：移除并重取
            rq.rbt.remove(id);
            TASKS[id].in_rq = false;
            if let Some(id2) = rq.rbt.min() {
                if TASKS[id2].state == TaskState::Ready {
                    next = id2;
                }
            }
        }
    }

    CURRENT = next;
    if next != 0 {
        TASKS[next].state = TaskState::Running;
    }
    TASKS[next].ctx_rsp as *mut ExceptionFrame
}

/// 当前任务睡眠 n 个 tick（自身上下文调用；hlt 让出直到被唤醒）。
pub fn sleep_ticks(n: u64) {
    // SAFETY: 任务上下文，单核。
    unsafe {
        crate::sync::cli();
        let cur = CURRENT;
        if cur == 0 {
            crate::sync::sti();
            return; // idle 不睡眠
        }
        TASKS[cur].sleep_until = TICKS.load(Ordering::Relaxed) + n;
        TASKS[cur].state = TaskState::Sleeping;
        if TASKS[cur].in_rq {
            crate::smp::cpu_rq(0).rbt.remove(cur);
            TASKS[cur].in_rq = false;
        }
        crate::sync::sti();
        // 让出 CPU：下个 tick 会在 IRQ 中完成切换；hlt 可被任何中断唤醒后返回。
        core::arch::asm!("hlt", options(nomem, nostack));
    }
}

/// 任务名（调试用）。
pub fn current_name() -> &'static str {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].name }
}

// ---- fork / exit / waitpid（M2 切片3）----

/// fork 汇编桩（boot.asm）：把返回地址传给 `rust_fork_impl`。
extern "C" {
    fn fork_wrapper() -> u32;
}

/// fork 当前任务：父返回子任务 id，子返回 0（Unix 语义）。
///
/// 实现：复制父内核栈已用部分到子新栈，在子栈构造初始帧
/// （rip = fork 返回地址，rax = 0），登记新任务；地址空间经 `vmm::on_fork` COW 共享。
pub fn fork(name: &'static str) -> u32 {
    // 记录名字：rust_fork_impl 无法从汇编桩传字符串，用静态暂存
    // SAFETY: 单核。
    unsafe { FORK_NAME = name };
    // SAFETY: fork_wrapper 为纯汇编桩。
    unsafe { fork_wrapper() }
}

/// fork 实现（由 fork_wrapper 调用；子任务帧恢复 fork 点寄存器快照）。
///
/// # Safety
/// 仅由 boot.asm fork_wrapper 调用。
/// `saved` 指向 fork_wrapper 压栈的 6 个 callee-saved 寄存器（r15,r14,r13,r12,rbx,rbp）；
/// `target_rsp` 为父任务 fork 调用返回后应有的 rsp（子任务以此恢复，局部变量/栈帧一致）。
#[no_mangle]
pub unsafe extern "C" fn rust_fork_impl(ret_addr: u64, saved: *const u64, target_rsp: u64) -> u32 {
    let cur = CURRENT;
    let cid = TASK_COUNT;
    if cid >= MAX_TASKS {
        return u32::MAX; // fork 失败（父侧）
    }

    // 分配子内核栈并复制父栈已用部分（[rsp, stack_top)）
    let layout = core::alloc::Layout::from_size_align(STACK_SIZE, 4096).unwrap();
    let cstack = alloc::alloc::alloc(layout);
    if cstack.is_null() {
        return u32::MAX;
    }
    let rsp: usize;
    core::arch::asm!(
        "movq %rsp, {0}",
        out(reg) rsp,
        options(nomem, nostack, att_syntax)
    );
    let cur_top = TASKS[cur].stack as usize + STACK_SIZE;
    let used = cur_top.saturating_sub(rsp);
    let c_top = cstack as usize + STACK_SIZE;
    let c_rsp = c_top - used;
    core::ptr::copy_nonoverlapping(rsp as *const u8, c_rsp as *mut u8, used);

    // 子任务初始帧：从 fork 返回地址继续，rsp=目标 rsp，rax=0（子侧返回 0），
    // 并恢复 fork 点的 callee-saved 寄存器（局部变量在寄存器中的值保持一致）。
    // 注意：rbp 若指向父栈（帧指针），必须按栈偏移平移，否则 rbp 相对寻址
    // 会读到父栈内容（真实内核 fork 的栈重定位逻辑）。
    let frame_ptr = (c_rsp - core::mem::size_of::<ExceptionFrame>()) & !15usize;
    let frame = &mut *(frame_ptr as *mut ExceptionFrame);
    *frame = core::mem::zeroed();
    frame.rip = ret_addr;
    frame.cs = 0x08;
    frame.ss = 0x10;
    frame.rflags = 0x202; // IF=1
    frame.rsp = c_rsp as u64 + (target_rsp - rsp as u64);
    frame.rax = 0;
    // 恢复 callee-saved 寄存器（saved[0..6] 顺序 = fork_wrapper 压栈顺序：
    // rbp, rbx, r12, r13, r14, r15 —— 即 [rsp+0]=rbp, [rsp+40]=r15）。
    // 任何指向父栈的值（含帧指针与栈上局部变量的指针）必须按栈偏移平移，
    // 否则子任务经这些寄存器访问父栈内容（或垃圾地址导致 #PF）。
    let parent_stack = TASKS[cur].stack as usize;
    let parent_top = parent_stack + STACK_SIZE;
    let stack_offset = cstack as isize - parent_stack as isize;
    let translate = |v: u64| -> u64 {
        let a = v as usize;
        if a >= parent_stack && a < parent_top {
            (a as isize + stack_offset) as u64
        } else {
            v
        }
    };
    let s = &*(saved as *const [u64; 6]);
    frame.r15 = translate(s[5]);
    frame.r14 = translate(s[4]);
    frame.r13 = translate(s[3]);
    frame.r12 = translate(s[2]);
    frame.rbx = translate(s[1]);
    frame.rbp = translate(s[0]);

    // 栈重定位：修正子栈副本中各帧存储的调用者 rbp（父栈地址 → 子栈偏移）。
    // 否则子任务经中间帧 `leave` 弹出未平移的 rbp 后，rbp 相对寻址会读到父栈内容。
    let offset = cstack as isize - parent_stack as isize;
    let mut fp = frame.rbp as usize;
    for _ in 0..64 {
        if fp < cstack as usize || fp + 8 > c_top {
            break;
        }
        // SAFETY: fp 在子栈副本内。
        let saved = *(fp as *const u64);
        if saved < parent_stack as u64 || saved >= parent_top as u64 {
            break; // 帧链结束（0 或非父栈指针）
        }
        let new_fp = (saved as isize + offset) as u64;
        *(fp as *mut u64) = new_fp;
        fp = new_fp as usize;
    }

    // 地址空间 COW 共享
    crate::vmm::on_fork(cid);

    TASKS[cid] = Task {
        name: FORK_NAME,
        state: TaskState::Ready,
        ctx_rsp: frame_ptr,
        stack: cstack,
        sleep_until: 0,
        base_prio: TASKS[cur].base_prio,
        effective_prio: TASKS[cur].effective_prio,
        vruntime: TASKS[cur].vruntime,
        run_ticks: 0,
        in_rq: false,
    };
    // 子任务入 CFS 就绪树（与父同 vruntime 起点，公平竞争）
    crate::smp::cpu_rq(0).rbt.insert(cid, TASKS[cur].vruntime);
    TASKS[cid].in_rq = true;
    TASK_COUNT += 1;
    cid as u32 // 父侧返回子 id
}

/// fork 名字暂存（fork → rust_fork_impl 单线程传递）。
static mut FORK_NAME: &'static str = "child";

/// 当前任务退出：释放地址空间，状态置 Exited（waitpid 可回收）。
pub fn exit() {
    // SAFETY: 单核。
    unsafe {
        let cur = CURRENT;
        crate::vmm::release_as(cur);
        TASKS[cur].state = TaskState::Exited;
        // 永不返回；不再被调度
        loop {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// 等待子任务退出（阻塞式；简化实现：轮询状态）。
pub fn waitpid(pid: usize) {
    loop {
        // SAFETY: 单核读。
        let state = unsafe { TASKS[pid].state };
        if state == TaskState::Exited {
            return;
        }
        sleep_ticks(1);
    }
}
