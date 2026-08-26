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
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;

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
    let nr = f.rax; // syscall 号
    let ret = dispatch(nr, f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9);
    f.rax = ret; // 返回值写回 rax
    frame
}

/// syscall 分派。
fn dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> u64 {
    match nr {
        SYS_WRITE => sys_write(a1, a2, a3),
        SYS_READ => sys_read(a1, a2, a3),
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

/// write(fd, buf, len)：仅支持 fd=1(stdout)→串口；返回写字节数。
fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    if fd == 1 {
        // SAFETY: buf 用户态地址，len 由用户保证。
        let data = unsafe { copy_from_user(buf, len as usize) };
        let s = core::str::from_utf8(data).unwrap_or("<non-utf8>");
        crate::print!("{}", s);
        len
    } else {
        (-1i64) as u64 // EBADF
    }
}

/// read(fd, buf, len)：M3 切片暂不支持串口读，返回 0（EOF）。
fn sys_read(_fd: u64, _buf: u64, _len: u64) -> u64 {
    0
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
