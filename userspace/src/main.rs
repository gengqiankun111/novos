//! Novos-OS 用户态 init/shell（M3 切片4）。
//!
//! no_std + no_main 的 freestanding 静态 ELF（ET_EXEC），由内核 ELF 加载器
//! 映射到 0x80_0000_0000 后经 `iretq` 进入。所有 I/O 走 Linux x86_64 ABI 的
//! `syscall` 指令（write=1 / read=0 / exit=60），无任何运行时依赖。

#![no_std]
#![no_main]

use core::panic::PanicInfo;

const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 60;

/// 通用 syscall（3 参数，Linux x86_64 约定：rax=nr, rdi/rsi/rdx=arg1-3）。
fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    // SAFETY: syscall 指令为 x86_64 标准接口；rcx/r11 被 CPU 覆盖。
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") nr => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

fn sys_write(fd: u64, buf: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
}

/// 非阻塞读一个字节（内核串口读）。
fn sys_read_byte() -> Option<u8> {
    let mut b = 0u8;
    let n = syscall3(SYS_READ, 0, core::ptr::addr_of_mut!(b) as u64, 1);
    if n == 1 { Some(b) } else { None }
}

fn print(s: &str) {
    sys_write(1, s.as_bytes());
}

/// 无 fmt 依赖的 u64 十进制打印。
fn print_u64(v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut n = v;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    // SAFETY: buf[i..] 全为 ASCII 数字。
    print(unsafe { core::str::from_utf8_unchecked(&buf[i..]) });
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("PANIC: userspace\n");
    syscall3(SYS_EXIT, 1, 0, 0);
    loop {
        // SAFETY: 停机等待内核/调试器。
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// 内建命令执行。
fn exec(cmd: &[u8]) {
    match cmd {
        [] => {}
        b"help" => {
            print("commands: help | echo <text> | exit | version\n");
        }
        b"version" => {
            print("Novos-OS userspace init v0.3.0 (M3)\n");
        }
        b"exit" => {
            print("bye\n");
            syscall3(SYS_EXIT, 0, 0, 0);
        }
        _ if cmd.starts_with(b"echo ") => {
            print(unsafe { core::str::from_utf8_unchecked(&cmd[5..]) });
            print("\n");
        }
        _ => {
            print("unknown: ");
            print(unsafe { core::str::from_utf8_unchecked(cmd) });
            print("\n");
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    print("\nNovos-OS M3 userspace shell (init)\n");
    print("type 'help' for commands\n");

    let mut line = [0u8; 128];
    let mut len = 0usize;
    let mut idle = 0u64;
    let mut prompted = false;

    loop {
        // 提示符只在每行开始打印一次（避免空轮询时刷屏）
        if len == 0 && !prompted {
            print("$ ");
            prompted = true;
        }
        match sys_read_byte() {
            Some(b) => {
                idle = 0;
                match b {
                    b'\r' | b'\n' => {
                        print("\n");
                        exec(&line[..len]);
                        len = 0;
                        prompted = false;
                    }
                    0x08 | 0x7F => {
                        if len > 0 {
                            len -= 1;
                            print("\x08 \x08");
                        }
                    }
                    b if b >= 0x20 && len < line.len() => {
                        line[len] = b;
                        len += 1;
                        // 回显当前字符
                        print(unsafe { core::str::from_utf8_unchecked(&[b]) });
                    }
                    _ => {}
                }
            }
            None => {
                idle += 1;
                // 无输入自动执行一次 demo，证明 shell 循环活跃（阈值足够大避免刷屏）
                if idle == 3_000_000 {
                    idle = 0;
                    print("\n[auto] ");
                    exec(b"help");
                    prompted = false;
                }
            }
        }
    }
}
