//! timerfd（M13-11）：`timerfd_create` + `timerfd_settime/gettime` + epoll 可监听。
//!
//! PIT 100Hz（10ms/tick，见 task::TICKS）→ nsec 换算 tick。到期后 fd 就绪
//! （epoll EPOLLIN），`read` 读回 u64 到期次数（一次/周期两种模式）。

use alloc::vec::Vec;
use crate::task;
use spin::Mutex;

/// timerfd fd 基址（fd = TIMER_FD_BASE + 表索引；大于 EPOLL_FD_BASE 300）。
pub const TIMER_FD_BASE: usize = 400;

/// 每 tick 纳秒数（PIT 100Hz）。
pub const NS_PER_TICK: u64 = 10_000_000;

/// 单条 timerfd 状态。
#[derive(Clone, Copy)]
pub struct TimerFd {
    pub fd: usize,
    /// 下一次到期 tick（0 = 未调度）。
    pub deadline: u64,
    /// 周期（tick）；0 = 一次性。
    pub interval: u64,
    /// 已到期未读次数。
    pub expirations: u64,
    /// 是否已启动（val != 0）。
    pub armed: bool,
}

static TIMERS: spin::Lazy<Mutex<Vec<TimerFd>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

pub fn is_timer_fd(fd: usize) -> bool {
    fd >= TIMER_FD_BASE
}

/// nsec → tick（负值/极小 → 0）。
fn nsec_to_ticks(sec: i64, nsec: i64) -> u64 {
    if sec < 0 || nsec < 0 {
        return 0;
    }
    (sec as u64) * 100 + (nsec as u64) / NS_PER_TICK
}

/// tick → itimerspec 单个 timespec { sec, nsec }。
fn ticks_to_timespec(t: u64) -> (i64, i64) {
    ((t / 100) as i64, ((t % 100) * NS_PER_TICK) as i64)
}

/// `timerfd_create(clockid, flags)`：创建 timerfd。
pub fn timerfd_create(_clockid: u64, _flags: u64) -> i64 {
    let mut t = TIMERS.lock();
    let fd = TIMER_FD_BASE + t.len();
    t.push(TimerFd {
        fd,
        deadline: 0,
        interval: 0,
        expirations: 0,
        armed: false,
    });
    fd as i64
}

/// `timerfd_settime(fd, flags, new_value, old_value)`：设/查（重）调度。
/// itimerspec { it_interval: {sec,nsec}, it_value: {sec,nsec} }（32 字节）。
pub fn timerfd_settime(fd: usize, _flags: u64, new: u64, old: u64) -> i64 {
    let (int_sec, int_nsec, val_sec, val_nsec) = if new != 0 {
        // SAFETY: new 为用户态 itimerspec（4×i64 可读）。
        let n = unsafe { core::ptr::read_volatile(new as *const [i64; 4]) };
        (n[0], n[1], n[2], n[3])
    } else {
        (0, 0, 0, 0)
    };
    let val_ticks = nsec_to_ticks(val_sec, val_nsec);
    let mut t = TIMERS.lock();
    let tm = match t.iter_mut().find(|x| x.fd == fd) {
        Some(x) => x,
        None => return -9, // EBADF
    };
    if old != 0 {
        let (is, ins) = ticks_to_timespec(tm.interval);
        let (vs, vns) = if tm.armed {
            let remain = tm.deadline.saturating_sub(task::ticks());
            let (s, ns) = ticks_to_timespec(remain);
            (s, ns)
        } else {
            (0, 0)
        };
        let o = [is, ins, vs, vns];
        // SAFETY: old 为用户态可写 itimerspec。
        unsafe { core::ptr::write_volatile(old as *mut [i64; 4], o) };
    }
    tm.interval = nsec_to_ticks(int_sec, int_nsec);
    tm.expirations = 0;
    if val_ticks == 0 {
        tm.armed = false;
        tm.deadline = 0;
    } else {
        tm.armed = true;
        tm.deadline = task::ticks() + val_ticks;
    }
    0
}

/// `timerfd_gettime(fd, curr_value)`：查询剩余时间与周期。
pub fn timerfd_gettime(fd: usize, curr: u64) -> i64 {
    let t = TIMERS.lock();
    let tm = match t.iter().find(|x| x.fd == fd) {
        Some(x) => x,
        None => return -9, // EBADF
    };
    let (is, ins) = ticks_to_timespec(tm.interval);
    let (vs, vns) = if tm.armed {
        let remain = tm.deadline.saturating_sub(task::ticks());
        let (s, ns) = ticks_to_timespec(remain);
        (s, ns)
    } else {
        (0, 0)
    };
    let o = [is, ins, vs, vns];
    // SAFETY: curr 为用户态可写 itimerspec。
    unsafe { core::ptr::write_volatile(curr as *mut [i64; 4], o) };
    0
}

/// PIT tick（IRQ 上下文，task::on_timer_tick 调用）：推进到期。
pub fn on_tick() {
    let now = task::ticks();
    let mut t = TIMERS.lock();
    for tm in t.iter_mut() {
        if !tm.armed {
            continue;
        }
        if now >= tm.deadline {
            tm.expirations += 1;
            if tm.interval > 0 {
                tm.deadline = now + tm.interval; // 周期模式：重新调度
            } else {
                tm.armed = false; // 一次性
            }
        }
    }
}

/// timerfd 就绪（epoll 用）：有未读到期次数。
pub fn timer_ready(fd: usize) -> bool {
    let t = TIMERS.lock();
    t.iter().find(|x| x.fd == fd).map_or(false, |x| x.expirations > 0)
}

/// read：读回到期次数（u64，8 字节）；未到期 EAGAIN。
pub fn timer_read(fd: usize, buf: u64) -> i64 {
    let mut t = TIMERS.lock();
    let tm = match t.iter_mut().find(|x| x.fd == fd) {
        Some(x) => x,
        None => return -9, // EBADF
    };
    if tm.expirations == 0 {
        return -11; // EAGAIN
    }
    let n = tm.expirations;
    tm.expirations = 0;
    // SAFETY: buf 为用户态可写 8 字节。
    unsafe { core::ptr::write_volatile(buf as *mut u64, n) };
    8
}

/// close：移除 timerfd。
pub fn timer_close(fd: usize) -> bool {
    let mut t = TIMERS.lock();
    let pos = t.iter().position(|x| x.fd == fd);
    if let Some(i) = pos {
        t.remove(i);
        return true;
    }
    false
}
