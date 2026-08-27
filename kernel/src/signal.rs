//! 信号子系统（M13-06）：`rt_sigaction` + 信号投递 + `rt_sigreturn` + SIGSEGV。
//!
//! 简化实现（Linux 语义对齐子集）：
//! - 每任务一组 disposition（handler/flags/mask）+ 挂起位图；
//! - 投递挂接点：syscall 返回（`deliver_if_pending`）与用户态 `#PF` 异常
//!   （`deliver_segv`，见 syscall.rs / interrupts.rs）；
//! - 信号帧为**自定义简化版**（与 userspace `sigtest` 严格对齐，非 glibc 全兼容）；
//! - `SA_SIGINFO` 提供 siginfo（signo/errno/code/addr）；默认动作 = 打印 + 终止任务。
//!
//! 信号帧 ABI（用户栈，16 对齐；handler 以 `rt_sigreturn` 收尾不返回）：
//! ```text
//! SigFrame @ frame_addr（rsp 指向 ret_addr，即 handler 的返回地址槽）
//!   +0x00 ret_addr
//!   +0x08 ucontext{flags,link,stack×3,sigmask}
//!   +0x38 saved: ExceptionFrame   （22×8=176B → 到 0xE8，rt_sigreturn 恢复的寄存器）
//!   +0xE8 siginfo{signo,errno,code,addr,pid,uid}
//! handler 入参：rdi=signo, rsi=&siginfo, rdx=&ucontext
//! ```

use crate::interrupts::ExceptionFrame;
use crate::task;
use core::sync::atomic::{AtomicU64, Ordering};

/// 信号编号（Linux 语义子集）。
pub const SIGSEGV: u64 = 11;
/// disposition：默认动作（终止）。
pub const SIG_DFL: u64 = 0;
/// disposition：忽略。
pub const SIG_IGN: u64 = 1;
/// `SA_*` flags。
pub const SA_SIGINFO: u64 = 0x4;
/// siginfo 的 si_code（SIGSEGV）。
pub const SEGV_MAPERR: u64 = 1;

/// 每任务信号状态（下标 = 任务 id，见 task::MAX_TASKS）。
#[derive(Clone, Copy)]
pub struct SigState {
    pub handler: u64,
    pub flags: u64,
    pub mask: u64,
    pub pending: u64,
    /// 最近一次投递的信号帧地址（rt_sigreturn 读取已修改的 mcontext）。
    pub frame_addr: u64,
}
const fn sig_state() -> SigState {
    SigState { handler: SIG_DFL, flags: 0, mask: 0, pending: 0, frame_addr: 0 }
}
static mut SIG: [SigState; task::MAX_TASKS] = [sig_state(); task::MAX_TASKS];

/// 用户态 `struct sigaction`（与 userspace sigtest 对齐）。
#[repr(C)]
pub struct SigAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

/// 信号帧（见模块头注释的 ABI）。
#[repr(C)]
pub struct SigFrame {
    pub ret_addr: u64,         // +0x00 rsp→
    pub uc_flags: u64,         // +0x08
    pub uc_link: u64,          // +0x10
    pub uc_stack_sp: u64,      // +0x18
    pub uc_stack_flags: u64,   // +0x20
    pub uc_stack_size: u64,    // +0x28
    pub uc_sigmask: u64,       // +0x30
    pub saved: ExceptionFrame, // +0x38（22×8=176B → 到 0xE8）
    pub si_signo: u64,         // +0xE8
    pub si_errno: u64,         // +0xF0
    pub si_code: u64,          // +0xF8
    pub si_addr: u64,          // +0x100
    pub si_pid: u64,           // +0x108
    pub si_uid: u64,           // +0x110
}

/// 当前任务信号状态的可变引用。
///
/// # Safety
/// 单核（关中断路径内调用）。
fn cur_state() -> &'static mut SigState {
    let id = task::current_id();
    // SAFETY: 单核独占访问静态表。
    unsafe { &mut SIG[id] }
}

/// `rt_sigaction(sig, act, oldact, sigsetsize)`：注册/查询信号 disposition。
pub fn sys_rt_sigaction(sig: u64, act: u64, oldact: u64, _sigsetsize: u64) -> i64 {
    if sig == 0 || sig > 31 {
        return -22; // EINVAL（常规信号 1–31）
    }
    let st = cur_state();
    if oldact != 0 {
        let old = SigAction { handler: st.handler, flags: st.flags, restorer: 0, mask: st.mask };
        // SAFETY: oldact 为已映射的用户地址（写回旧 disposition）。
        unsafe { core::ptr::write_volatile(oldact as *mut SigAction, old) };
    }
    if act != 0 {
        // SAFETY: act 为已映射的用户地址。
        let a = unsafe { core::ptr::read_volatile(act as *const SigAction) };
        st.handler = a.handler;
        st.flags = a.flags;
        st.mask = a.mask;
    }
    0
}

/// 投递信号：改 `f` 使返回用户态时进入 handler。
/// 返回 true = 已投递（f 已被改写）；false = 未投递（忽略/无挂起）。
pub fn deliver(f: &mut ExceptionFrame, signo: u64, si_code: u64, si_addr: u64) -> bool {
    let st = cur_state();
    let bit = 1u64 << (signo - 1);
    st.pending |= bit;
    // SIGSEGV 不可忽略（Linux 语义：忽略等价默认终止）
    if st.handler == SIG_IGN && signo != SIGSEGV {
        st.pending &= !bit;
        return false;
    }
    if st.handler == SIG_IGN || st.handler == SIG_DFL {
        st.pending &= !bit;
        // 默认动作：打印 + 终止任务（task::exit 永不返回）
        crate::println!("signal {}: default action, task terminating", signo);
        task::exit();
        return false; // 不可达
    }
    // 有用户 handler：构建信号帧并改写 f
    st.pending &= !bit;
    let frame_addr = (f.rsp - core::mem::size_of::<SigFrame>() as u64) & !15u64;
    st.frame_addr = frame_addr as u64;
    // SAFETY: 用户栈页已映射（恒等映射可写）。
    unsafe {
        let fp = frame_addr as *mut SigFrame;
        core::ptr::write_volatile(
            fp,
            SigFrame {
                ret_addr: 0,
                uc_flags: 0,
                uc_link: 0,
                uc_stack_sp: 0,
                uc_stack_flags: 0,
                uc_stack_size: 0,
                uc_sigmask: st.mask,
                saved: *f,
                si_signo: signo,
                si_errno: 0,
                si_code,
                si_addr,
                si_pid: task::current_pid() as u64,
                si_uid: 0,
            },
        );
    }
    // 进入 handler（C ABI）：rdi=signo, rsi=&siginfo(+0xE8), rdx=&ucontext(+0x08)
    f.rdi = signo;
    f.rsi = frame_addr as u64 + 0xE8;
    f.rdx = frame_addr as u64 + 0x08;
    f.rip = st.handler;
    f.rsp = frame_addr as u64;
    true
}

/// 有挂起信号则投递最低编号信号（syscall/irq 返回前调用）。
pub fn deliver_if_pending(f: &mut ExceptionFrame) -> bool {
    let pend = cur_state().pending;
    if pend == 0 {
        return false;
    }
    let signo = pend.trailing_zeros() as u64 + 1;
    deliver(f, signo, 0, 0)
}

/// 用户态 `#PF` → SIGSEGV（interrupts 调用）。
pub fn deliver_segv(f: &mut ExceptionFrame, addr: u64) -> bool {
    deliver(f, SIGSEGV, SEGV_MAPERR, addr)
}

/// `rt_sigreturn`：从用户栈信号帧恢复被中断的上下文。
/// 帧地址取投递时保存的 `frame_addr`（handler 执行期间 rsp 已不在帧基址）。
pub fn sys_rt_sigreturn(f: &mut ExceptionFrame) {
    let frame_addr = cur_state().frame_addr;
    if frame_addr == 0 {
        return; // 无信号帧：忽略
    }
    let saved_addr = frame_addr + 0x38; // SigFrame.saved 偏移
    // SAFETY: 用户栈信号帧已映射。
    let saved: ExceptionFrame = unsafe { core::ptr::read_volatile(saved_addr as *const ExceptionFrame) };
    // 恢复全部通用寄存器 + 段/栈/标志
    f.r15 = saved.r15;
    f.r14 = saved.r14;
    f.r13 = saved.r13;
    f.r12 = saved.r12;
    f.r11 = saved.r11;
    f.r10 = saved.r10;
    f.r9 = saved.r9;
    f.r8 = saved.r8;
    f.rbp = saved.rbp;
    f.rdi = saved.rdi;
    f.rsi = saved.rsi;
    f.rdx = saved.rdx;
    f.rcx = saved.rcx;
    f.rbx = saved.rbx;
    f.rax = saved.rax;
    f.rip = saved.rip;
    f.cs = saved.cs;
    f.rflags = saved.rflags;
    f.rsp = saved.rsp;
    f.ss = saved.ss;
}
