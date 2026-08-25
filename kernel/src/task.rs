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
        let cur = CURRENT;
        TASKS[cur].state = TaskState::Blocked;
        core::arch::asm!("hlt", options(nomem, nostack));
    }
}

/// 唤醒一个阻塞/睡眠任务（由 sync 原语调用）。
pub fn wake(id: usize) {
    // SAFETY: 单核写。
    unsafe {
        if id != 0
            && (TASKS[id].state == TaskState::Blocked || TASKS[id].state == TaskState::Sleeping)
        {
            TASKS[id].state = TaskState::Ready;
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
        };
        Ok(idx as u32)
    }
}

/// PIT tick（IRQ 上下文调用，IF 关闭）：保存当前任务 → 唤醒到期睡眠任务 →
/// 按有效优先级选择下一个任务（同优先级轮转）→ 返回其帧指针。
///
/// # Safety
/// 仅由 rust_irq_handler 调用，单核且中断关闭。
pub unsafe fn on_timer_tick(frame: *mut ExceptionFrame) -> *mut ExceptionFrame {
    let cur = CURRENT;
    TASKS[cur].ctx_rsp = frame as usize;
    if TASKS[cur].state == TaskState::Running {
        TASKS[cur].state = TaskState::Ready;
    }

    TICKS.fetch_add(1, Ordering::Relaxed);
    let now = TICKS.load(Ordering::Relaxed);

    // 唤醒到期睡眠任务
    for i in 1..TASK_COUNT {
        if TASKS[i].state == TaskState::Sleeping && TASKS[i].sleep_until <= now {
            TASKS[i].state = TaskState::Ready;
        }
    }

    // 最高有效优先级（PIP 提升后参与选择）
    let mut best = 0u8;
    for i in 1..TASK_COUNT {
        if TASKS[i].state == TaskState::Ready && TASKS[i].effective_prio > best {
            best = TASKS[i].effective_prio;
        }
    }

    // 在最高优先级集合内轮转（从 cur 之后找第一个）
    let mut next = 0; // 默认 idle
    if best > 0 {
        for j in 1..=MAX_TASKS {
            let cand = (cur + j) % MAX_TASKS;
            if cand != 0
                && cand < TASK_COUNT
                && TASKS[cand].state == TaskState::Ready
                && TASKS[cand].effective_prio == best
            {
                next = cand;
                break;
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
        let cur = CURRENT;
        if cur == 0 {
            return; // idle 不睡眠
        }
        TASKS[cur].sleep_until = TICKS.load(Ordering::Relaxed) + n;
        TASKS[cur].state = TaskState::Sleeping;
        // 让出 CPU：下个 tick 会在 IRQ 中完成切换；hlt 可被任何中断唤醒后返回。
        core::arch::asm!("hlt", options(nomem, nostack));
    }
}

/// 任务名（调试用）。
pub fn current_name() -> &'static str {
    // SAFETY: 单核读。
    unsafe { TASKS[CURRENT].name }
}
