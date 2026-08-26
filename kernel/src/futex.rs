//! M11-切片1：futex 系统调用（WAIT/WAKE 共享内存同步原语）。
//!
//! 简化设计（demo 规模）：
//! - 全局等待表 { 用户地址 → 任务 id }（线性表，数量少）；
//! - FUTEX_WAIT：*addr == val 时阻塞当前任务，否则返回 EAGAIN(11)；
//! - FUTEX_WAKE：唤醒 addr 上所有等待任务，返回唤醒数。

use alloc::vec::Vec;
use spin::Mutex;

/// futex 等待表：{ 用户地址, 任务 id }。
static FUTEX_WAITERS: spin::Lazy<Mutex<Vec<(usize, usize)>>> =
    spin::Lazy::new(|| Mutex::new(Vec::new()));

/// futex(addr, op, val)。
///
/// # Safety
/// 由 syscall 处理器调用（IF=0）；`uaddr` 为用户态共享内存地址（恒等映射）。
pub unsafe fn futex(uaddr: u64, op: u64, val: u64) -> u64 {
    match op {
        0 => {
            let cur = core::ptr::read_volatile(uaddr as *const u32);
            if cur != val as u32 {
                return 11; // EAGAIN
            }
            FUTEX_WAITERS.lock().push((uaddr as usize, crate::task::current_id()));
            crate::task::block_current();
            0
        }
        1 => {
            let mut w = FUTEX_WAITERS.lock();
            let mut n = 0u64;
            let mut i = 0;
            while i < w.len() {
                if w[i].0 == uaddr as usize {
                    let tid = w[i].1;
                    crate::task::wake(tid);
                    w.remove(i);
                    n += 1;
                } else {
                    i += 1;
                }
            }
            n
        }
        _ => (-22i64) as u64, // EINVAL
    }
}
