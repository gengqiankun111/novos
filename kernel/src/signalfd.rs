//! signalfd（M13-12）：`signalfd4` + epoll 可监听 + read 读 siginfo。
//!
//! 语义简化：信号生成（kill）时若匹配某 signalfd 的 mask，则被 signalfd
//! 消费（不再走 handler/默认动作），`read` 读回 32 字节 signalfd_siginfo
//! （前 8 字段与 Linux `struct signalfd_siginfo` 布局一致）。

use alloc::vec::Vec;
use spin::Mutex;

/// signalfd fd 基址（fd = SIGNALFD_FD_BASE + 表索引；大于 TIMER_FD_BASE 400）。
pub const SIGNALFD_FD_BASE: usize = 500;

/// signalfd_siginfo 前 32 字节（Linux ABI 兼容：ssi_signo 起 8 个字段）。
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SignalfdSiginfo {
    pub signo: u32,
    pub errno: i32,
    pub code: i32,
    pub pid: u32,
    pub uid: u32,
    pub fd: i32,
    pub tid: u32,
    pub band: u32,
}

/// 单条 signalfd 状态（固定容量环形消费队列，8 条足够测试/事件循环）。
#[derive(Clone, Copy)]
pub struct Signalfd {
    pub fd: usize,
    /// 监听信号集（bit sig-1）。
    pub mask: u64,
    pub queue: [SignalfdSiginfo; 8],
    pub qlen: usize,
}

static SIGNALFDS: spin::Lazy<Mutex<Vec<Signalfd>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

fn empty_info() -> SignalfdSiginfo {
    SignalfdSiginfo { signo: 0, errno: 0, code: 0, pid: 0, uid: 0, fd: 0, tid: 0, band: 0 }
}

pub fn is_signalfd(fd: usize) -> bool {
    fd >= SIGNALFD_FD_BASE
}

/// `signalfd4(fd, mask, flags)`：fd = u64::MAX → 新建；否则更新已有 mask。
pub fn signalfd4(fd: u64, mask: u64, _flags: u64) -> i64 {
    let mut s = SIGNALFDS.lock();
    if fd != u64::MAX {
        match s.iter_mut().find(|x| x.fd == fd as usize) {
            Some(x) => {
                x.mask = mask;
                return fd as i64;
            }
            None => return -9, // EBADF
        }
    }
    let nfd = SIGNALFD_FD_BASE + s.len();
    s.push(Signalfd { fd: nfd, mask, queue: [empty_info(); 8], qlen: 0 });
    nfd as i64
}

/// 信号生成钩子（`signal::sys_kill` 调用）：mask 覆盖则入队并消费。
/// 返回 true = 已被 signalfd 消费（调用方应清除 pending，不再默认动作/投递）。
pub fn notify(sig: u64) -> bool {
    if !(1..=31).contains(&sig) {
        return false;
    }
    let bit = 1u64 << (sig - 1);
    let mut s = SIGNALFDS.lock();
    let mut consumed = false;
    for sfd in s.iter_mut() {
        if sfd.mask & bit == 0 {
            continue;
        }
        let info = SignalfdSiginfo {
            signo: sig as u32,
            errno: 0,
            code: 0,
            pid: crate::task::current_pid(),
            uid: 0,
            fd: 0,
            tid: 0,
            band: 0,
        };
        if sfd.qlen < sfd.queue.len() {
            sfd.queue[sfd.qlen] = info;
            sfd.qlen += 1;
        }
        consumed = true;
    }
    consumed
}

/// signalfd 就绪（epoll 用）：队列非空。
pub fn signalfd_ready(fd: usize) -> bool {
    let s = SIGNALFDS.lock();
    s.iter().find(|x| x.fd == fd).map_or(false, |x| x.qlen > 0)
}

/// read：读回 32 字节 signalfd_siginfo（FIFO 弹出队首）；空队列 EAGAIN。
pub fn signalfd_read(fd: usize, buf: u64) -> i64 {
    let mut s = SIGNALFDS.lock();
    let sfd = match s.iter_mut().find(|x| x.fd == fd) {
        Some(x) => x,
        None => return -9, // EBADF
    };
    if sfd.qlen == 0 {
        return -11; // EAGAIN
    }
    let info = sfd.queue[0];
    for i in 1..sfd.qlen {
        sfd.queue[i - 1] = sfd.queue[i];
    }
    sfd.qlen -= 1;
    // SAFETY: buf 为用户态可写 32 字节。
    unsafe { core::ptr::write_volatile(buf as *mut SignalfdSiginfo, info) };
    core::mem::size_of::<SignalfdSiginfo>() as i64
}

/// close：移除 signalfd。
pub fn signalfd_close(fd: usize) -> bool {
    let mut s = SIGNALFDS.lock();
    let pos = s.iter().position(|x| x.fd == fd);
    if let Some(i) = pos {
        s.remove(i);
        return true;
    }
    false
}
