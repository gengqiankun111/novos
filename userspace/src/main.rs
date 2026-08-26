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
const SYS_OPEN: u64 = 2;
const SYS_CLOSE: u64 = 3;
const SYS_MKDIR: u64 = 83;
const SYS_RMDIR: u64 = 84;
const SYS_UNLINK: u64 = 87;
const SYS_EXIT: u64 = 60;
const SYS_GETDENTS64: u64 = 217;

// open flags（Linux O_*）
const O_CREAT: u64 = 0o100;
const O_TRUNC: u64 = 0o1000;

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

/// 从命令参数提取 NUL 结尾路径（trim 尾随空白）。
fn path_arg(cmd: &[u8], start: usize) -> [u8; 128] {
    let mut p = [0u8; 128];
    let mut end = cmd.len();
    while end > start && (cmd[end - 1] == b' ' || cmd[end - 1] == b'\t') {
        end -= 1;
    }
    let n = core::cmp::min(end - start, p.len() - 1);
    p[..n].copy_from_slice(&cmd[start..start + n]);
    p
}

/// 枚举目录并打印（Linux dirent64 格式，目录加 "/" 后缀；循环读至 EOF）。
fn list_dir(p: &[u8]) {
    let fd = syscall3(SYS_OPEN, p.as_ptr() as u64, 0, 0);
    if (fd as i64) < 0 {
        print("ls: open failed\n");
        return;
    }
    loop {
        let mut buf = [0u8; 1024];
        let n = syscall3(SYS_GETDENTS64, fd, buf.as_mut_ptr() as u64, 1024);
        if n == 0 {
            break;
        }
        let mut off = 0usize;
        while off + 19 <= n as usize {
            let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            let typ = buf[off + 18];
            let mut nl = 0usize;
            while off + 19 + nl < buf.len() && buf[off + 19 + nl] != 0 {
                nl += 1;
            }
            if nl > 0 {
                // SAFETY: 目录项名称为 ASCII。
                let nm =
                    unsafe { core::str::from_utf8_unchecked(&buf[off + 19..off + 19 + nl]) };
                print(nm);
                if typ == 4 {
                    print("/ ");
                } else {
                    print("  ");
                }
            }
            off += reclen;
        }
    }
    print("\n");
    syscall3(SYS_CLOSE, fd, 0, 0);
}

/// 内建命令执行。
fn exec(cmd: &[u8]) {
    match cmd {
        [] => {}
        b"help" => {
            print("commands: help | ls [dir] | cat <f> | echo <text> | mkdir <d> | rm <f> | rmdir <d> | version | fdtest | fstest | dtest | exit\n");
        }
        b"version" => {
            print("Novos-OS userspace init v0.3.0 (M3)\n");
        }
        b"fdtest" => {
            // 验证 fd 表 + open/close：打开 /dev/uart（应得 fd 3），写入，关闭
            let path = b"/dev/uart\0";
            let fd = syscall3(SYS_OPEN, path.as_ptr() as u64, 0, 0);
            if (fd as i64) < 0 {
                print("fdtest: open failed\n");
            } else {
                print("fdtest: opened /dev/uart fd=");
                print_u64(fd);
                print("\n");
                syscall3(SYS_WRITE, fd, "fdtest: hello via open fd\n".as_ptr() as u64, 27);
                let r = syscall3(SYS_CLOSE, fd, 0, 0);
                print("fdtest: close rc=");
                print_u64(r);
                print("\n");
            }
        }
        b"fstest" => {
            // M4 切片1：ramfs 创建文件 → 写入 → 读回验证
            let path = b"/etc/motd\0";
            let fd = syscall3(SYS_OPEN, path.as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
            if (fd as i64) < 0 {
                print("fstest: create failed rc=");
                print_u64(fd);
                print("\n");
            } else {
                let msg = b"hello from ramfs\n";
                syscall3(SYS_WRITE, fd, msg.as_ptr() as u64, msg.len() as u64);
                syscall3(SYS_CLOSE, fd, 0, 0);
                // 读回
                let fd2 = syscall3(SYS_OPEN, path.as_ptr() as u64, 0, 0);
                let mut buf = [0u8; 64];
                let n = syscall3(SYS_READ, fd2, buf.as_mut_ptr() as u64, 64);
                print("fstest: read ");
                print_u64(n);
                print("B: ");
                print(unsafe { core::str::from_utf8_unchecked(&buf[..n as usize]) });
                syscall3(SYS_CLOSE, fd2, 0, 0);
            }
        }
        b"dtest" => {
            // M4-切片3：dcache shrink 验收——创建 1000 个文件触发回收
            let dir = b"/dtest\0";
            syscall3(SYS_MKDIR, dir.as_ptr() as u64, 0o755, 0); // 已存在 EEXIST，忽略
            let prefix = b"/dtest/f";
            for i in 0..1000u32 {
                let mut p = [0u8; 24];
                p[..8].copy_from_slice(prefix);
                let mut n = i;
                let mut digits = [0u8; 4];
                let mut dlen = 0usize;
                loop {
                    digits[dlen] = b'0' + (n % 10) as u8;
                    dlen += 1;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
                for k in 0..dlen {
                    p[8 + k] = digits[dlen - 1 - k];
                }
                p[8 + dlen] = 0;
                let fd = syscall3(SYS_OPEN, p.as_ptr() as u64, O_CREAT | 1, 0o644);
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
            print("dtest: created 1000 files under /dtest\n");
        }
        b"exit" => {
            print("bye\n");
            syscall3(SYS_EXIT, 0, 0, 0);
        }
        b"ls" => {
            list_dir(b"/\0");
        }
        _ if cmd.starts_with(b"ls ") => {
            let p = path_arg(cmd, 3);
            list_dir(&p);
        }
        _ if cmd.starts_with(b"cat ") => {
            let p = path_arg(cmd, 4);
            let fd = syscall3(SYS_OPEN, p.as_ptr() as u64, 0, 0);
            if (fd as i64) < 0 {
                print("cat: open failed\n");
            } else {
                loop {
                    let mut buf = [0u8; 128];
                    let n = syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, 128);
                    if n == 0 {
                        break;
                    }
                    // SAFETY: 文件内容按 ASCII 输出。
                    print(unsafe { core::str::from_utf8_unchecked(&buf[..n as usize]) });
                }
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
        }
        _ if cmd.starts_with(b"mkdir ") => {
            let p = path_arg(cmd, 6);
            let rc = syscall3(SYS_MKDIR, p.as_ptr() as u64, 0o755, 0);
            if rc != 0 {
                print("mkdir: rc=");
                print_u64(rc);
                print("\n");
            }
        }
        _ if cmd.starts_with(b"rmdir ") => {
            let p = path_arg(cmd, 6);
            let rc = syscall3(SYS_RMDIR, p.as_ptr() as u64, 0, 0);
            if rc != 0 {
                print("rmdir: rc=");
                print_u64(rc);
                print("\n");
            }
        }
        _ if cmd.starts_with(b"rm ") => {
            let p = path_arg(cmd, 3);
            let rc = syscall3(SYS_UNLINK, p.as_ptr() as u64, 0, 0);
            if rc != 0 {
                print("rm: rc=");
                print_u64(rc);
                print("\n");
            }
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
