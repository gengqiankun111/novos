//! M12：Linux capability 集（capget/capset）。
//!
//! 模型与 Linux 对齐：每进程 effective/permitted/inheritable 三集合，
//! 按位表示 capability。本内核无 CAP_SETPCAP 提权路径，因此 capset
//! 只允许**降权**（新集合必须是当前集合的子集），否则返回 EPERM。

use crate::task;

/// Linux `_LINUX_CAPABILITY_VERSION_3`。
pub const CAP_VERSION_3: u32 = 0x2008_0522;

/// `struct __user_cap_header_struct { __u32 version; int pid; }`（8 字节）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapHeader {
    pub version: u32,
    pub pid: i32,
}

/// `struct __user_cap_data_struct { __u32 effective, permitted, inheritable; }`（12 字节）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapData {
    pub effective: u32,
    pub permitted: u32,
    pub inheritable: u32,
}

/// 解析 pid 参数：pid=0 表示当前进程；其它非当前 pid 一律拒绝（单用户内核）。
fn resolve_pid(pid: i32) -> i64 {
    let cur = task::current_pid() as i32;
    if pid == 0 || pid == cur {
        0
    } else {
        -1 // EPERM
    }
}

/// capget(hdr, data)：读取进程 capability 集。
pub fn sys_capget(hdr: u64, data: u64) -> i64 {
    if hdr == 0 || data == 0 {
        return -14; // EFAULT
    }
    // SAFETY: hdr 为用户态指针，8 字节可读（capsetest 对齐的栈结构）。
    let h = unsafe { core::ptr::read_volatile(hdr as *const CapHeader) };
    if h.version != CAP_VERSION_3 {
        return -22; // EINVAL
    }
    if resolve_pid(h.pid) != 0 {
        return -1; // EPERM
    }
    let caps = task::current_caps();
    let d = CapData {
        effective: caps[0],
        permitted: caps[1],
        inheritable: caps[2],
    };
    // SAFETY: data 为用户态指针，12 字节可写。
    unsafe { core::ptr::write_volatile(data as *mut CapData, d) };
    0
}

/// capset(hdr, data)：设置 capability 集（仅允许降权）。
pub fn sys_capset(hdr: u64, data: u64) -> i64 {
    if hdr == 0 || data == 0 {
        return -14; // EFAULT
    }
    // SAFETY: hdr 为用户态指针，8 字节可读。
    let h = unsafe { core::ptr::read_volatile(hdr as *const CapHeader) };
    if h.version != CAP_VERSION_3 {
        return -22; // EINVAL
    }
    if resolve_pid(h.pid) != 0 {
        return -1; // EPERM
    }
    // SAFETY: data 为用户态指针，12 字节可读。
    let d = unsafe { core::ptr::read_volatile(data as *const CapData) };
    let cur = task::current_caps();
    // 降权校验：任何集合的新位不在当前集合中 → 拒绝（本内核无提权路径）。
    if (d.effective & !cur[0]) != 0 || (d.permitted & !cur[1]) != 0 || (d.inheritable & !cur[2]) != 0 {
        return -1; // EPERM
    }
    task::set_caps([d.effective, d.permitted, d.inheritable]);
    0
}
