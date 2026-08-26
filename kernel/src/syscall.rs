//! M3 切片2：syscall 框架——配置 syscall MSR + 系统调用分派 + 基础系统调用。
//!
//! syscall 指令进入 `syscall_entry`（boot.asm），构造 ExceptionFrame 后调
//! `rust_syscall_handler`；返回时修改 frame.rax 为返回值。
//! 系统调用号遵循 Linux x86_64 ABI（read=0/write=1/getpid=39/exit=60）。

use crate::interrupts::ExceptionFrame;
use core::arch::asm;

// ---- MSR 编号 ----
const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_FMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1 << 0; // SysCall Enable

// ---- 系统调用号（Linux x86_64 ABI）----
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;
pub const SYS_MOUNT: u64 = 165;
pub const SYS_GETDENTS64: u64 = 217;

/// boot.asm 导出的 syscall 入口。
extern "C" {
    fn syscall_entry();
}

fn wrmsr(msr: u32, value: u64) {
    // SAFETY: wrmsr 为特权指令；msr 为合法编号。
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack)
        );
    }
}

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdmsr 为特权指令。
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | lo as u64
}

/// 配置 syscall/sysret MSR。
///
/// # Safety
/// 仅启动阶段调用一次。
pub fn init() {
    // EFER.SCE = 1
    let efer = rdmsr(MSR_EFER);
    wrmsr(MSR_EFER, efer | EFER_SCE);
    // STAR[47:32]=0x08(kcode), STAR[63:48]=0x10(kdata→sysret 偏移到 user 段)
    wrmsr(MSR_STAR, (0x10u64 << 48) | (0x08u64 << 32));
    // LSTAR = syscall_entry 地址
    wrmsr(MSR_LSTAR, syscall_entry as usize as u64);
    // FMASK：syscall 时清 IF（bit9），处理期间关中断
    wrmsr(MSR_FMASK, 0x200);
}

/// 系统调用统一入口（由 boot.asm syscall_entry 调用）。
///
/// # Safety
/// 仅由 syscall_entry 以有效帧指针调用；此时已在内核栈。
#[no_mangle]
pub unsafe extern "C" fn rust_syscall_handler(frame: *mut ExceptionFrame) -> *mut ExceptionFrame {
    // SAFETY: frame 由 syscall_entry 构造。
    let f = unsafe { &mut *frame };
    // syscall_entry 用 r12 暂存用户 RSP，已把用户 r12 溢出到用户栈 [rsp-8]：
    // 修复帧中的 rsp（恢复原值）与 r12（取回溢出值），保证 syscall 透传所有寄存器。
    let user_rsp = f.rsp + 8;
    // SAFETY: f.rsp 指向用户栈上 syscall_entry 溢出保存的用户 r12（用户页已映射）。
    let user_r12 = unsafe { *(f.rsp as *const u64) };
    f.r12 = user_r12;
    f.rsp = user_rsp;
    let nr = f.rax; // syscall 号
    // M5：每次系统调用轮询一次网络收包（无中断依赖）
    crate::net::net_poll();
    let ret = dispatch(nr, f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9);
    f.rax = ret; // 返回值写回 rax
    frame
}

/// syscall 分派。
fn dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> u64 {
    match nr {
        SYS_WRITE => sys_write(a1, a2, a3),
        SYS_READ => sys_read(a1, a2, a3),
        SYS_OPEN => sys_open(a1, a2, a3),
        SYS_CLOSE => sys_close(a1),
        SYS_STAT => sys_stat(a1, a2),
        SYS_MKDIR => sys_mkdir(a1, a2),
        SYS_RMDIR => sys_rmdir(a1),
        SYS_UNLINK => sys_unlink(a1),
        SYS_MOUNT => sys_mount(a1, a2, a3, a4, a5),
        SYS_GETDENTS64 => sys_getdents64(a1, a2, a3),
        SYS_GETPID => sys_getpid(),
        SYS_EXIT => sys_exit(a1),
        _ => (-1i64) as u64, // ENOSYS
    }
}

/// 用户态内存只读拷贝（M3 切片：恒等映射，用户地址直接可读；后续加边界检查）。
///
/// # Safety
/// src 为用户态地址，len 不越界由调用方保证。
unsafe fn copy_from_user(src: u64, len: usize) -> &'static [u8] {
    // SAFETY: 恒等映射下用户地址即物理地址；M3 首个用户态进程内存由内核映射。
    unsafe { core::slice::from_raw_parts(src as *const u8, len) }
}

/// 拷贝用户态 NUL 结尾字符串（最多 dst.len()-1 字节）。
///
/// # Safety
/// src 为用户态地址；dst 长度受调用方约束。
unsafe fn copy_cstr_from_user(src: u64, dst: &mut [u8]) -> usize {
    // SAFETY: 逐字节读用户地址直至 NUL 或 dst 满。
    unsafe {
        for i in 0..dst.len() {
            let c = *((src as *const u8).add(i));
            if c == 0 {
                return i;
            }
            dst[i] = c;
        }
        dst.len() // 无 NUL：按满长截断（不报错，M3 简化）
    }
}

/// write(fd, buf, len)：经 fd 表路由到文件（0/1/2 = uart）；返回写字节数。
fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    // SAFETY: buf 用户态地址，len 由用户保证。
    let data = unsafe { copy_from_user(buf, len as usize) };
    match crate::fs::fd_get(fd as usize) {
        Some(f) => f.write(data) as u64,
        None => (-1i64) as u64, // EBADF
    }
}

/// read(fd, buf, len)：经 fd 表路由到文件；非阻塞，无数据返回 0。
/// 临时缓冲 512B 上限（文件按 len 读；uart 逐字节）。
fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    let mut tmp = [0u8; 512];
    let want = core::cmp::min(len as usize, tmp.len());
    match crate::fs::fd_get(fd as usize) {
        Some(f) => {
            let n = f.read(&mut tmp[..want]);
            if n > 0 {
                // SAFETY: buf 为用户态地址且 len ≥ n；用户页已映射，恒等映射下可写。
                unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n) };
            }
            n as u64
        }
        None => (-1i64) as u64, // EBADF
    }
}

/// open(path, flags, mode)："/dev/uart" 或 ramfs 文件（O_CREAT 创建）；成功返回新 fd。
fn sys_open(path: u64, flags: u64, _mode: u64) -> u64 {
    let mut pbuf = [0u8; 256];
    // SAFETY: path 为用户态 NUL 结尾字符串。
    let n = unsafe { copy_cstr_from_user(path, &mut pbuf) };
    // SAFETY: pbuf[..n] 为合法 UTF-8（ASCII 路径）。
    let name = unsafe { core::str::from_utf8_unchecked(&pbuf[..n]) };
    match crate::fs::open_path(name, flags) {
        Ok(f) => crate::fs::fd_alloc(f) as u64,
        Err(e) => e as u64, // 负 errno（按 Linux ABI 返回 -errno）
    }
}

/// close(fd)：关闭并回收 fd；成功返回 0。
fn sys_close(fd: u64) -> u64 {
    if crate::fs::fd_close(fd as usize) {
        0
    } else {
        (-1i64) as u64 // EBADF
    }
}

/// 拷贝用户态路径到 String，返回名称。
fn copy_path(path: u64) -> Result<alloc::string::String, u64> {
    let mut pbuf = [0u8; 256];
    // SAFETY: path 为用户态 NUL 结尾字符串。
    let n = unsafe { copy_cstr_from_user(path, &mut pbuf) };
    // SAFETY: pbuf[..n] 为合法 UTF-8（ASCII 路径）。
    Ok(alloc::string::String::from(unsafe {
        core::str::from_utf8_unchecked(&pbuf[..n])
    }))
}

/// mkdir(path, mode)：创建目录；成功 0，失败负 errno。
fn sys_mkdir(path: u64, _mode: u64) -> u64 {
    match copy_path(path) {
        Ok(name) => match crate::fs::create_dir(&name) {
            Ok(()) => 0,
            Err(e) => e as u64,
        },
        Err(e) => e,
    }
}

/// rmdir(path)：删除空目录；成功 0，失败负 errno。
fn sys_rmdir(path: u64) -> u64 {
    match copy_path(path) {
        Ok(name) => match crate::fs::remove(&name, true) {
            Ok(()) => 0,
            Err(e) => e as u64,
        },
        Err(e) => e,
    }
}

/// unlink(path)：删除文件；成功 0，失败负 errno。
fn sys_unlink(path: u64) -> u64 {
    match copy_path(path) {
        Ok(name) => match crate::fs::remove(&name, false) {
            Ok(()) => 0,
            Err(e) => e as u64,
        },
        Err(e) => e,
    }
}

/// getdents64(fd, buf, len)：枚举目录到用户缓冲，返回字节数（0 = 结束）。
fn sys_getdents64(fd: u64, buf: u64, len: u64) -> u64 {
    match crate::fs::fd_get(fd as usize) {
        Some(f) => match f.as_ref() {
            crate::fs::File::Dir { inode, pos } => {
                // SAFETY: buf 为用户态地址且 len 由用户保证；内核侧枚举到临时缓冲。
                let mut tmp = [0u8; 1024];
                let start = *pos.lock();
                let (n, items) =
                    crate::fs::read_dir(inode.as_ref(), start, &mut tmp).unwrap_or((0, 0));
                *pos.lock() = start + items; // 游标前进
                let want = core::cmp::min(n, len as usize);
                // SAFETY: 拷贝 tmp[..want] 到用户 buf（len ≥ want）。
                unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, want) };
                want as u64
            }
            _ => (-1i64) as u64, // ENOTDIR
        },
        None => (-1i64) as u64, // EBADF
    }
}

/// stat(path, buf)：填 Linux x86_64 stat 关键字段（st_ino/nlink/mode/size）。
fn sys_stat(path: u64, buf: u64) -> u64 {
    let name = match copy_path(path) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let (ino, mode, nlink, size) = match crate::fs::stat_path(&name) {
        Ok(v) => v,
        Err(e) => return e as u64,
    };
    let mut tmp = [0u8; 144];
    tmp[8..16].copy_from_slice(&ino.to_le_bytes()); // st_ino
    tmp[16..24].copy_from_slice(&nlink.to_le_bytes()); // st_nlink
    tmp[24..28].copy_from_slice(&(mode as u32).to_le_bytes()); // st_mode
    tmp[48..56].copy_from_slice(&(size as i64).to_le_bytes()); // st_size
    // SAFETY: buf 为用户态地址（144 字节可写）。
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, 144) };
    0
}

/// mount(source, target, fstype, flags, data)：M4-切片4 简化——忽略 source/fstype，
/// 挂载新 tmpfs 根到 target。
fn sys_mount(source: u64, target: u64, _fstype: u64, _flags: u64, _data: u64) -> u64 {
    let _src = match copy_path(source) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let tgt = match copy_path(target) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match crate::fs::mount_fs(&tgt) {
        Ok(()) => 0,
        Err(e) => e as u64,
    }
}

/// getpid()：返回当前任务 id（M3 切片：固定 1）。
fn sys_getpid() -> u64 {
    1
}

/// exit(code)：用户态进程退出——M3 切片先打印并停机（真正的 exit 待进程模型完善）。
fn sys_exit(code: u64) -> u64 {
    crate::println!("[syscall] user process exit({code})");
    // SAFETY: hlt 特权指令，用户态退出后停机（M3 切片简化）。
    unsafe { asm!("hlt", options(nomem, nostack)) };
    0 // 不可达
}

/// 供启动日志确认 syscall 就绪。
pub fn info() -> &'static str {
    "syscall(msr/table) ready"
}
