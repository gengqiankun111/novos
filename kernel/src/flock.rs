//! M14：最小字节区间记录锁（fcntl `F_SETLK`/`F_SETLKW`/`F_GETLK`/`F_UNLCK`，
//! SQLite 依赖，DESIGN §13.12）。
//!
//! 语义（Linux 子集）：
//! - 锁按 **inode + 进程 pid** 管理；区间为 `[start, end)`（`l_len=0` 表示到 EOF）；
//! - **写锁**与任何重叠锁冲突；**读锁**仅与写锁冲突；
//! - `F_SETLK` 非阻塞（冲突返回 EAGAIN），`F_SETLKW` 阻塞（sleep 重试）；
//! - `F_GETLK` 返回冲突锁信息（无冲突时 `l_type` 置 `F_UNLCK`）；
//! - 同进程重复加锁为"替换"语义（先移除同区间旧锁）。

use alloc::vec::Vec;
use spin::Mutex;

/// fcntl 命令（Linux x86_64）。
pub const F_GETLK: u64 = 5;
pub const F_SETLK: u64 = 6;
pub const F_SETLKW: u64 = 7;
/// 锁类型。
pub const F_RDLCK: i16 = 0;
pub const F_WRLCK: i16 = 1;
pub const F_UNLCK: i16 = 2;
/// whence。
pub const SEEK_SET: i16 = 0;
pub const SEEK_END: i16 = 2;

/// 活动锁条目。
#[derive(Clone, Copy)]
struct LockEntry {
    inode: usize,
    typ: i16,
    start: u64,
    end: u64, // 独占上界（l_len=0 → u64::MAX）
    pid: u32,
}

static LOCKS: spin::Lazy<Mutex<Vec<LockEntry>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

/// 区间重叠。
fn overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// 两锁是否冲突：非 UNLCK + 区间重叠 + 至少一方是写锁。
fn conflicts(t1: i16, r1: (u64, u64), t2: i16, r2: (u64, u64)) -> bool {
    if t1 == F_UNLCK || t2 == F_UNLCK {
        return false;
    }
    if !overlap(r1, r2) {
        return false;
    }
    t1 == F_WRLCK || t2 == F_WRLCK
}

/// 把 whence + 偏移解析为绝对区间（文件大小 size）。
fn resolve_range(typ: i16, whence: i16, start: i64, len: i64, size: u64) -> Option<(u64, u64)> {
    if start < 0 {
        return None; // 负偏移不支持（最小实现）
    }
    let base = match whence {
        SEEK_SET => 0u64,
        SEEK_END => size,
        _ => return None,
    };
    let s = base.checked_add(start as u64)?;
    let end = if len == 0 {
        u64::MAX
    } else if len < 0 {
        return None; // 负长度不支持
    } else {
        s.checked_add(len as u64)?
    };
    let _ = typ;
    Some((s, end))
}

/// 尝试加锁/解锁（inode 为常规文件指针；size 用于 SEEK_END 解析）。
/// 返回 0 或 -11（EAGAIN）。
pub fn set_lock(
    inode: usize,
    typ: i16,
    whence: i16,
    start: i64,
    len: i64,
    pid: u32,
    size: u64,
    blocking: bool,
) -> i64 {
    let (s, e) = match resolve_range(typ, whence, start, len, size) {
        Some(r) => r,
        None => return -22, // EINVAL
    };
    loop {
        let mut l = LOCKS.lock();
        let conflicted = l.iter().any(|x| {
            x.inode == inode && x.pid != pid && conflicts(x.typ, (x.start, x.end), typ, (s, e))
        });
        if !conflicted {
            if typ == F_UNLCK {
                // 移除该 inode 上同进程重叠区间的旧锁
                l.retain(|x| !(x.inode == inode && x.pid == pid && overlap((x.start, x.end), (s, e))));
            } else {
                // 替换语义：先移除同进程重叠旧锁，再插入
                l.retain(|x| !(x.inode == inode && x.pid == pid && overlap((x.start, x.end), (s, e))));
                l.push(LockEntry { inode, typ, start: s, end: e, pid });
            }
            return 0;
        }
        if !blocking {
            return -11; // EAGAIN
        }
        // 阻塞等待：sleep_deadline 提供真实睡眠窗口（rq 空出 → 持锁进程可运行解锁）。
        // 注：不能用 sleep_ticks——其单次 hlt 会让本进程在下个 tick 立即重新入队，
        // rq 永不为空，父进程（task 0 仅 rq 空时被选）得不到调度 → 死锁（见 problem_solving P9）。
        drop(l);
        crate::task::sleep_deadline(crate::task::ticks() + 2);
    }
}

/// F_GETLK：返回冲突锁（type/start/end/pid）；无冲突返回 None。
pub fn get_lock(
    inode: usize,
    typ: i16,
    whence: i16,
    start: i64,
    len: i64,
    pid: u32,
    size: u64,
) -> Option<(i16, u64, u64, u32)> {
    let (s, e) = resolve_range(typ, whence, start, len, size)?;
    let l = LOCKS.lock();
    for x in l.iter() {
        if x.inode == inode && x.pid != pid && conflicts(x.typ, (x.start, x.end), typ, (s, e)) {
            return Some((x.typ, x.start, x.end, x.pid));
        }
    }
    None
}

/// fd 关闭时释放该 inode 上指定进程的全部锁（POSIX fcntl 语义）。
pub fn release_inode(pid: u32, inode: usize) {
    LOCKS.lock().retain(|x| !(x.inode == inode && x.pid == pid));
}

/// 当前活动锁条数（测试/调试）。
pub fn lock_count() -> usize {
    LOCKS.lock().len()
}
