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
    /// 用户页表根（CR3 值；用户任务有效，0 = 内核任务）。
    pub cr3: usize,
    /// pid（ns 内编号）。
    pub pid: u32,
    /// pid namespace id（0 = 根）。
    pub pid_ns: u32,
    /// uts namespace id（0 = 根）。
    pub uts_ns: u32,
    /// cgroup id（0 = 根；pids/内存记账用）。
    pub cgroup: u32,
    /// 当前工作目录（NUL 结尾绝对路径；chdir 修改，M8-切片1）。
    pub cwd: [u8; 64],
    /// TLS 段基址（FS base；M11-切片2 arch_prctl，随任务切换保存/恢复）。
    pub fs_base: u64,
    /// clone flags（M11-切片3：CLONE_SETTLS/CLONE_CHILD_CLEARTID 等）。
    pub flags: u32,
    /// CLONE_CHILD_CLEARTID 目标地址：退出时清零并 futex 唤醒（pthread_join）。
    pub child_tidptr: u64,
    /// Linux capability 集（M12：effective/permitted/inheritable）。
    pub caps: [u32; 3],
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
            cr3: 0,
            pid: 0,
            pid_ns: 0,
            uts_ns: 0,
            cgroup: 0,
            cwd: [0; 64],
            fs_base: 0,
            flags: 0,
            child_tidptr: 0,
            caps: [0; 3],
        }
    }
}

/// 当前任务 capability 集（M12：effective/permitted/inheritable）。
pub fn current_caps() -> [u32; 3] {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].caps }
}

/// 设置当前任务 capability 集（capset 用）。
pub fn set_caps(c: [u32; 3]) {
    // SAFETY: 单核写。
    unsafe { TASKS[CURRENT].caps = c; }
}

/// 根 cwd = "/"。
fn cwd_root() -> [u8; 64] {
    let mut c = [0u8; 64];
    c[0] = b'/';
    c
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

/// 唤醒阻塞/睡眠任务（M11-切片4：futex 带超时等待被显式 WAKE 时用）。
pub fn wake_any(id: usize) {
    // SAFETY: 单核写；cli 保护树操作。
    unsafe {
        if id != 0
            && (TASKS[id].state == TaskState::Blocked || TASKS[id].state == TaskState::Sleeping)
        {
            TASKS[id].state = TaskState::Ready;
            if !TASKS[id].in_rq {
                crate::smp::cpu_rq(0).rbt.insert(id, TASKS[id].vruntime);
                TASKS[id].in_rq = true;
            }
        }
    }
}

/// 带截止时刻的阻塞（M11-切片4：futex WAIT 超时）：置 Sleeping + sleep_until，
/// hlt 直到被 tick 唤醒（截止到）或 futex WAKE 显式唤醒。
pub fn sleep_deadline(deadline: u64) {
    // SAFETY: 任务上下文，单核。
    unsafe {
        crate::sync::cli();
        let cur = CURRENT;
        if cur == 0 {
            // task 0（init/shell 内联执行命令）：on_timer_tick 唤醒循环从 i=1 开始、
            // 不覆盖 task 0，故不能置 Sleeping；改为 hlt 自旋等 tick 到点。
            // volatile 读 TICKS：hlt 带 options(nomem) 时编译器不得提升负载。
            crate::sync::sti();
            while unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!(TICKS) as *const AtomicU64)
                    .load(Ordering::Relaxed)
            } < deadline
            {
                core::arch::asm!("hlt", options(nomem, nostack));
            }
            return;
        }
        TASKS[cur].sleep_until = deadline;
        TASKS[cur].state = TaskState::Sleeping;
        if TASKS[cur].in_rq {
            crate::smp::cpu_rq(0).rbt.remove(cur);
            TASKS[cur].in_rq = false;
        }
        crate::sync::sti();
        // hlt 可被任何中断唤醒后返回；未到截止/未被 WAKE 时继续等。
        // state 用 volatile 读：hlt 带 options(nomem)，否则编译器会把
        // TASKS[cur].state 提升到循环外（tick 唤醒不再可见）→ 死循环。
        while unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(TASKS[cur].state) as *const TaskState)
        } == TaskState::Sleeping
        {
            core::arch::asm!("hlt", options(nomem, nostack));
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
            cr3: 0,
            pid: 0,
            pid_ns: 0,
            uts_ns: 0,
            cgroup: 0,
            cwd: cwd_root(),
            fs_base: 0,
            flags: 0,
            child_tidptr: 0,
            caps: [0; 3],
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
    // M13-11：timerfd 到期推进（IRQ 上下文，关中断，天然互斥）。
    crate::timer::on_tick();

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
    for i in 1..MAX_TASKS {
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
    // M6-切片1：切到不同任务时更新 TSS.RSP0（用户态中断入口栈）与 CR3（用户页表）。
    // task 0（init/shell）用固定 syscall 栈；fork 出的用户任务用各自内核栈。
    if next != cur {
        let rsp0 = if next == 0 {
            crate::gdt::init_rsp0()
        } else {
            TASKS[next].stack as usize + STACK_SIZE
        };
        crate::gdt::set_rsp0(rsp0);
        let cr3 = TASKS[next].cr3;
        if cr3 != 0 {
            // SAFETY: CR3 切换为特权指令；目标为用户进程页表（含内核恒等映射）。
            core::arch::asm!("mov cr3, {0}", in(reg) cr3, options(nostack, nomem));
        }
        // M11-切片2：恢复目标任务的 TLS（FS base）——用户态 %fs 寻址依赖。
        // SAFETY: 单核 + 关中断（tick 上下文）。
        unsafe {
            restore_fs_base(next);
        }
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

/// 当前任务用户页表根（CR3；内核任务为 0）。
pub fn current_cr3() -> usize {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].cr3 }
}

// ---- fork / exit / waitpid（M2 切片3）----

/// fork 汇编桩（boot.asm）：把返回地址传给 `rust_fork_impl`。
#[cfg(not(test))]
extern "C" {
    fn fork_wrapper() -> u32;
}
// host 单测桩：boot.asm 不参与测试链接（fork 不会被单测调用，返回 0 占位）。
#[cfg(test)]
fn fork_wrapper() -> u32 {
    0
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
        cr3: 0,
        pid: 0,
        pid_ns: 0,
        uts_ns: 0,
        cgroup: 0,
        cwd: TASKS[cur].cwd,
        fs_base: TASKS[cur].fs_base,
        flags: 0,
        child_tidptr: 0,
        caps: TASKS[cur].caps,
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
        // M11-切片3：CLONE_CHILD_CLEARTID——退出时把 tid 清零并 futex 唤醒，
        // pthread_join 借此得知线程已结束。
        let tidp = TASKS[cur].child_tidptr;
        if TASKS[cur].flags & 0x0020_0000 != 0 && tidp != 0 {
            // SAFETY: 用户地址恒等映射。
            unsafe { core::ptr::write_volatile(tidp as *mut u32, 0) };
            crate::futex::futex(tidp, 1, 1, 0, 0); // WAKE 1 个
        }
        crate::vmm::release_as(cur);
        TASKS[cur].state = TaskState::Exited;
        // M6-切片1：用户子进程从 syscall（IF=0）退出，须先开中断，
        // 否则 hlt 不会被 PIT 唤醒，系统卡死。
        crate::sync::sti();
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

// ---- 用户态 fork / pid namespace（M6-切片1）----

/// 当前任务的 pid（ns 内编号）。
pub fn current_pid() -> u32 {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].pid }
}

/// 当前任务的 pid namespace id。
pub fn current_pid_ns() -> u32 {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].pid_ns }
}

/// 指定 pid ns 内所有任务 pid（M8-切片4：/proc 视图）。
/// 说明：task 0（init/shell）state 恒为 Idle，但始终存活，需显式包含；
/// 槽位经 waitpid 回收后复用（TASK_COUNT 不随之增长），故遍历全表。
pub fn tasks_in_ns(pid_ns: u32) -> alloc::vec::Vec<u32> {
    // SAFETY: 单核读。
    unsafe {
        (0..MAX_TASKS)
            .filter(|&i| {
                TASKS[i].pid_ns == pid_ns
                    && (TASKS[i].state != TaskState::Idle || i == 0)
            })
            .map(|i| TASKS[i].pid)
            .collect()
    }
}

/// 容器数：非根 pid ns 且有存活任务（M9-切片1：/proc/health）。
pub fn container_count() -> u32 {
    let mut seen = [false; 16];
    let mut n = 0u32;
    // SAFETY: 单核读。
    unsafe {
        for i in 1..MAX_TASKS {
            let ns = TASKS[i].pid_ns as usize;
            if ns != 0 && TASKS[i].state != TaskState::Idle && !seen[ns] {
                seen[ns] = true;
                n += 1;
            }
        }
    }
    n
}

/// 所有任务累计运行 tick 总和（M9-切片1：/proc/health CPU 负载近似）。
pub fn busy_ticks() -> u64 {
    // SAFETY: 单核读。
    unsafe { (1..MAX_TASKS).map(|i| TASKS[i].run_ticks).sum() }
}

/// 登记当前任务的下次恢复帧（fork 子先执行时父帧）。
pub fn save_ctx(frame: usize) {
    // SAFETY: 单核。
    unsafe { TASKS[CURRENT].ctx_rsp = frame }
}

/// 设置当前运行任务（fork 子先执行时切到子任务）。
pub fn set_current(id: usize) {
    // SAFETY: 单核。
    unsafe { CURRENT = id }
}

/// 任务 ctx_rsp（fork 返回子帧用）。
pub fn task_ctx(id: usize) -> usize {
    // SAFETY: 单核读。
    unsafe { TASKS[id].ctx_rsp }
}

/// 任务内核栈顶（fork 子先执行时切换 tss_rsp0 用）。
pub fn task_kstack_top(id: usize) -> usize {
    // SAFETY: 单核读。
    unsafe {
        if id == 0 {
            crate::gdt::init_rsp0()
        } else {
            TASKS[id].stack as usize + STACK_SIZE
        }
    }
}

/// 注册用户 shell 的 CR3 与根 pid（task 0 进入 ring3 前调用）。
pub fn register_user_task(cr3: usize) {
    // SAFETY: 单核。
    unsafe {
        TASKS[0].cr3 = cr3;
        TASKS[0].pid = 1; // 根 ns 的 init（pid 1）
        TASKS[0].pid_ns = 0;
        TASKS[0].cwd = cwd_root();
        // M12：init（pid 1）以全量 Linux capability 集启动，fork 子进程继承。
        TASKS[0].caps = [0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF];
    }
}

/// 当前任务 cwd（NUL 结尾绝对路径）。
pub fn current_cwd() -> &'static [u8] {
    // SAFETY: 单核读。
    unsafe {
        let c = &TASKS[CURRENT].cwd;
        let len = c.iter().position(|&b| b == 0).unwrap_or(c.len());
        &c[..len]
    }
}

/// 设置当前任务 cwd（chdir 用；path 为已校验存在的绝对目录路径）。
pub fn set_cwd(path: &str) -> Result<(), i64> {
    if path.len() >= 63 {
        return Err(-36i64); // ENAMETOOLONG
    }
    let mut buf = [0u8; 64];
    buf[..path.len()].copy_from_slice(path.as_bytes());
    // SAFETY: 单核写。
    unsafe {
        TASKS[CURRENT].cwd = buf;
    }
    Ok(())
}

// ---- TLS（M11-切片2：arch_prctl ARCH_SET_FS/ARCH_GET_FS）----

/// 写 FS base MSR。
///
/// # Safety
/// wrmsr 为特权指令。
unsafe fn wrmsr_fs_base(v: u64) {
    // SAFETY: MSR_FS_BASE=0xC0000100。
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") 0xC000_0100u32,
            in("eax") v as u32,
            in("edx") (v >> 32) as u32,
            options(nomem, nostack)
        );
    }
}

/// 设置当前任务 TLS 段基址（FS base），并写入 MSR。
pub fn set_fs_base(v: u64) {
    // SAFETY: 单核写 + wrmsr 特权指令。
    unsafe {
        TASKS[CURRENT].fs_base = v;
        wrmsr_fs_base(v);
    }
}

/// 当前任务 TLS 段基址（FS base）。
pub fn get_fs_base() -> u64 {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].fs_base }
}

/// 切任务时恢复目标任务的 FS base（与 CR3/RSP0 一并由调度器切换）。
///
/// # Safety
/// 在 on_timer_tick（关中断）中调用。
unsafe fn restore_fs_base(id: usize) {
    // SAFETY: 单核 + 关中断。
    unsafe {
        wrmsr_fs_base(TASKS[id].fs_base);
    }
}

/// pid namespace 表：ns id → 下一个可分配 pid（0 号固定根 ns）。
static mut NS_NEXT_PID: [u32; 16] = [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
static mut NEXT_NS: u32 = 1;

// ---- uts namespace（M6-切片2）----

/// uts namespace 表：ns id → hostname（0 号固定根 = "shanshui-guanxin"）。
static mut UTS_HOST: [[u8; 32]; 8] = [
    *b"shanshui-guanxin\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    [0; 32],
    [0; 32],
    [0; 32],
    [0; 32],
    [0; 32],
    [0; 32],
    [0; 32],
];
static mut NEXT_UTS_NS: u32 = 1;

/// sethostname：设置当前任务 uts ns 的 hostname。
pub fn sethostname(name: &[u8]) -> i64 {
    let ns = {
        // SAFETY: 单核读。
        unsafe { TASKS[CURRENT].uts_ns as usize }
    };
    let n = core::cmp::min(name.len(), 31);
    // SAFETY: 单核写 UTS 表。
    unsafe {
        for (i, b) in name[..n].iter().enumerate() {
            UTS_HOST[ns][i] = *b;
        }
        UTS_HOST[ns][n] = 0;
    }
    0
}

/// 当前任务 uts ns 的 hostname（NUL 结尾）。
pub fn gethostname() -> &'static [u8] {
    let ns = {
        // SAFETY: 单核读。
        unsafe { TASKS[CURRENT].uts_ns as usize }
    };
    // SAFETY: 单核读静态表。
    unsafe {
        let h = &UTS_HOST[ns];
        let len = h.iter().position(|&b| b == 0).unwrap_or(h.len());
        &h[..len]
    }
}

// ---- cgroup v2 简化（M6-切片3）----

/// cgroup 表：每项 { pids, 记账内存 }（0 号根）。
static mut CGROUP_TASKS: [u32; 8] = [1, 0, 0, 0, 0, 0, 0, 0];
static mut CGROUP_MEM: [u64; 8] = [0, 0, 0, 0, 0, 0, 0, 0];

/// 当前任务 cgroup id。
pub fn current_cgroup() -> u32 {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].cgroup }
}

/// 读 cgroup 统计（{ pids, mem }，供 syscall 填回用户缓冲）。
pub fn cgroup_stat(cg: u32) -> (u64, u64) {
    let cg = (cg as usize) & 7;
    // SAFETY: 单核读。
    unsafe { (CGROUP_TASKS[cg] as u64, CGROUP_MEM[cg]) }
}

/// fork 记账：子入父 cgroup，pids+1，子内核栈 64KB 计入 cgroup 内存。
fn cgroup_charge(cg: u32) {
    let cg = (cg as usize) & 7;
    // SAFETY: 单核写。
    unsafe {
        CGROUP_TASKS[cg] += 1;
        CGROUP_MEM[cg] += STACK_SIZE as u64;
    }
}

/// 回收记账：pids-1，内存-64KB（与 charge 配对，无泄漏）。
fn cgroup_uncharge(cg: u32) {
    let cg = (cg as usize) & 7;
    // SAFETY: 单核写。
    unsafe {
        CGROUP_TASKS[cg] = CGROUP_TASKS[cg].saturating_sub(1);
        CGROUP_MEM[cg] = CGROUP_MEM[cg].saturating_sub(STACK_SIZE as u64);
    }
}

/// 分配一个空闲任务槽（优先复用已回收槽，其次追加）。
fn alloc_task_slot() -> Option<usize> {
    // SAFETY: 单核。
    unsafe {
        for i in 1..MAX_TASKS {
            if TASKS[i].state == TaskState::Idle {
                return Some(i);
            }
        }
        if TASK_COUNT < MAX_TASKS {
            let cid = TASK_COUNT;
            TASK_COUNT += 1;
            return Some(cid);
        }
        None
    }
}

/// 用户态 fork/clone：复制当前用户上下文到新任务（子返回 0），
/// 子任务经调度器（PIT 抢占）随后在用户态继续执行。
///
/// `flags` 支持：
/// - CLONE_NEWPID（0x20000000）：子进入新 pid ns，pid = 1。
/// - CLONE_NEWUTS（0x04000000）：子进入新 uts ns（hostname 独立）。
/// - CLONE_SETTLS（0x00080000）：`tls` 即子的 FS base（pthread_create TLS）。
/// - CLONE_CHILD_CLEARTID（0x00200000）：子退出时清零 `child_tidptr`
///   并 futex 唤醒（pthread_join 依赖）。
///
/// # Safety
/// 由 syscall 处理器调用；`frame` 为当前 syscall 的用户上下文（固定 syscall 栈上）。
pub unsafe fn user_fork(
    frame: *const crate::interrupts::ExceptionFrame,
    flags: u32,
    child_tidptr: u64,
    tls: u64,
) -> i64 {
    let cur = CURRENT;
    let cid = match alloc_task_slot() {
        Some(c) => c,
        None => return -1,
    };
    // 子内核栈：PIT 在用户态抢占时经 TSS.RSP0 使用
    let layout = core::alloc::Layout::from_size_align(STACK_SIZE, 4096).unwrap();
    let cstack = alloc::alloc::alloc(layout);
    if cstack.is_null() {
        return -1;
    }
    // 子初始帧 = 父 syscall 帧拷贝（用户上下文一致，rax 置 0 = 子侧 fork 返回 0）
    let frame_ptr = (cstack as usize + STACK_SIZE - core::mem::size_of::<crate::interrupts::ExceptionFrame>())
        & !15usize;
    let dst = &mut *(frame_ptr as *mut crate::interrupts::ExceptionFrame);
    *dst = *frame;
    dst.rax = 0;

    // pid namespace：CLONE_NEWPID → 新 ns 且子 pid=1；否则沿用父 ns 顺序号
    let (pid, pid_ns) = if flags & 0x2000_0000 != 0 {
        let ns = NEXT_NS;
        NEXT_NS = NEXT_NS.wrapping_add(1) & 0xF;
        (1u32, ns)
    } else {
        let ns = TASKS[cur].pid_ns;
        let pid = NS_NEXT_PID[ns as usize];
        NS_NEXT_PID[ns as usize] = pid.wrapping_add(1);
        (pid, ns)
    };
    // uts namespace：CLONE_NEWUTS → 新 uts ns（hostname 独立）；否则共享父 ns
    let uts_ns = if flags & 0x0400_0000 != 0 {
        let ns = NEXT_UTS_NS;
        NEXT_UTS_NS = NEXT_UTS_NS.wrapping_add(1) & 0x7;
        ns
    } else {
        TASKS[cur].uts_ns
    };

    // cgroup：子继承父 cgroup（Linux 语义），pids/内存记账
    let cg = TASKS[cur].cgroup;
    cgroup_charge(cg);

    TASKS[cid] = Task {
        name: "user-child",
        state: TaskState::Ready,
        ctx_rsp: frame_ptr,
        stack: cstack,
        sleep_until: 0,
        base_prio: TASKS[cur].base_prio,
        effective_prio: TASKS[cur].effective_prio,
        vruntime: TASKS[cur].vruntime,
        run_ticks: 0,
        in_rq: false,
        cr3: TASKS[cur].cr3, // 与父共享用户页表（内存隔离留后续切片）
        pid,
        pid_ns,
        uts_ns,
        cgroup: cg,
        cwd: TASKS[cur].cwd,
        // CLONE_SETTLS：子的 TLS 直接取 clone 参数（pthread_create 新线程
        // 从自己的 TLS 起步）；否则继承父（fork 语义）。
        fs_base: if flags & 0x0008_0000 != 0 { tls } else { TASKS[cur].fs_base },
        flags,
        child_tidptr,
        caps: TASKS[cur].caps,
    };
    // 子任务入 CFS 就绪树（同 vruntime 起点）
    crate::smp::cpu_rq(0).rbt.insert(cid, TASKS[cur].vruntime);
    TASKS[cid].in_rq = true;
    cid as i64 // 父侧返回子任务 id
}

/// 非阻塞 waitpid：子已 Exited 则回收槽并返回 pid；否则返回 0。
pub fn waitpid_nb(pid: usize) -> i64 {
    if pid == 0 || pid >= MAX_TASKS {
        return -1;
    }
    // SAFETY: 单核。
    unsafe {
        if TASKS[pid].state == TaskState::Exited {
            // 释放子内核栈（槽可复用）并归还 cgroup 记账
            if !TASKS[pid].stack.is_null() {
                let layout =
                    core::alloc::Layout::from_size_align(STACK_SIZE, 4096).unwrap();
                alloc::alloc::dealloc(TASKS[pid].stack, layout);
                TASKS[pid].stack = ptr::null_mut();
            }
            cgroup_uncharge(TASKS[pid].cgroup);
            TASKS[pid].state = TaskState::Idle;
            pid as i64
        } else {
            0
        }
    }
}
