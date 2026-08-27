//! M11-切片1/4：futex 系统调用（WAIT/WAKE + REQUEUE/CMP_REQUEUE + 超时）。
//!
//! 简化设计（demo 规模）：
//! - 全局等待表 { 用户地址, 任务 id, 截止 tick }（线性表，数量少）；
//! - FUTEX_WAIT：*addr == val 时阻塞，否则返回 EAGAIN(11)；带超时则到点
//!   返回 ETIMEDOUT(110)（deadline 0 = 无限等待）；
//! - FUTEX_WAKE：唤醒 addr 上至多 val 个等待任务，返回唤醒数；
//! - FUTEX_REQUEUE：唤醒 addr 上 val 个，把其余 val2 个搬到 addr2；
//! - FUTEX_CMP_REQUEUE：*addr == val2 时同 REQUEUE，否则返回 EAGAIN(11)。

use alloc::vec::Vec;
use spin::Mutex;

/// futex 等待条目：{ 用户地址, 任务 id, 截止 tick（0 = 无限） }。
struct Waiter {
    addr: usize,
    task: usize,
    deadline: u64,
}

/// futex 等待表。
static FUTEX_WAITERS: spin::Lazy<Mutex<Vec<Waiter>>> =
    spin::Lazy::new(|| Mutex::new(Vec::new()));

/// futex(addr, op, val, arg4, arg5)。
///
/// - op 0 WAIT：*addr == val 才阻塞；arg4 非 0 时是超时 tick 数（0 = 无限）。
/// - op 1 WAKE：唤醒至多 val 个。
/// - op 3 REQUEUE：唤醒 val 个，再搬至多 arg5 个到 arg4（目标地址）。
/// - op 4 CMP_REQUEUE：*addr == arg5 时唤醒 val 个、把其余全搬 arg4，否则 EAGAIN。
///
/// # Safety
/// 由 syscall 处理器调用（IF=0）；`addr` 为用户态共享内存地址（恒等映射）。
pub unsafe fn futex(addr: u64, op: u64, val: u64, arg4: u64, arg5: u64) -> u64 {
    let now = crate::task::ticks();
    match op {
        0 => {
            let cur = core::ptr::read_volatile(addr as *const u32);
            if cur != val as u32 {
                return 11; // EAGAIN
            }
            let task = crate::task::current_id();
            if arg4 == 0 {
                // 无限等待：Blocked，直到 futex WAKE 显式唤醒
                FUTEX_WAITERS.lock().push(Waiter {
                    addr: addr as usize,
                    task,
                    deadline: 0,
                });
                crate::task::block_current();
                0
            } else {
                // 超时等待：Sleeping + sleep_until，tick 到点或 WAKE 唤醒
                let deadline = now + arg4;
                FUTEX_WAITERS.lock().push(Waiter {
                    addr: addr as usize,
                    task,
                    deadline,
                });
                crate::task::sleep_deadline(deadline);
                // 醒来后若仍在等待表（未被 WAKE 移除）→ 超时
                let mut w = FUTEX_WAITERS.lock();
                if let Some(pos) = w.iter().position(|e| e.addr == addr as usize && e.task == task) {
                    w.remove(pos);
                    110 // ETIMEDOUT
                } else {
                    0
                }
            }
        }
        1 => {
            // WAKE：唤醒至多 val 个
            let mut w = FUTEX_WAITERS.lock();
            let mut n = 0u64;
            let mut i = 0;
            while i < w.len() && n < val {
                if w[i].addr == addr as usize {
                    crate::task::wake_any(w[i].task);
                    w.remove(i);
                    n += 1;
                } else {
                    i += 1;
                }
            }
            n
        }
        3 | 4 => {
            // REQUEUE / CMP_REQUEUE：CMP 先校验 *addr == arg5
            if op == 4 && core::ptr::read_volatile(addr as *const u32) != arg5 as u32 {
                return 11; // EAGAIN
            }
            let mut w = FUTEX_WAITERS.lock();
            let mut n = 0u64; // 唤醒数
            let mut moved = 0u64; // 搬家数
            let mut i = 0;
            while i < w.len() {
                if w[i].addr != addr as usize {
                    i += 1;
                    continue;
                }
                if n < val {
                    // 先唤醒 val 个
                    crate::task::wake_any(w[i].task);
                    w.remove(i);
                    n += 1;
                } else if op == 3 && moved >= arg5 {
                    // REQUEUE：已搬满 arg5 个，跳过其余
                    i += 1;
                } else {
                    // REQUEUE 搬至多 arg5 个；CMP_REQUEUE 搬全部（保留截止时刻）
                    w[i].addr = arg4 as usize;
                    i += 1;
                    moved += 1;
                }
            }
            n + moved
        }
        _ => (-22i64) as u64, // EINVAL
    }
}
