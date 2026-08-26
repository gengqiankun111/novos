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
const SYS_STAT: u64 = 4;
const SYS_MKDIR: u64 = 83;
const SYS_RMDIR: u64 = 84;
const SYS_UNLINK: u64 = 87;
const SYS_EXIT: u64 = 60;
const SYS_SOCKET: u64 = 41;
const SYS_CONNECT: u64 = 42;
const SYS_ACCEPT: u64 = 43;
const SYS_SENDTO: u64 = 44;
const SYS_RECVFROM: u64 = 45;
const SYS_BIND: u64 = 49;
const SYS_LISTEN: u64 = 50;
const SYS_EPOLL_CREATE: u64 = 213;
const SYS_EPOLL_WAIT: u64 = 232;
const SYS_EPOLL_CTL: u64 = 233;
const SYS_CLONE: u64 = 56;
const SYS_FORK: u64 = 57;
const SYS_GETPID: u64 = 39;
const SYS_WAITPID: u64 = 61;
const SYS_MOUNT: u64 = 165;
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

/// 通用 syscall（5 参数，arg4/arg5 走 r10/r8）。
fn syscall5(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64 {
    let ret: u64;
    // SAFETY: syscall 指令为 x86_64 标准接口；rcx/r11 被 CPU 覆盖。
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") nr => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    ret
}

/// 通用 syscall（6 参数，arg4-6 走 r10/r8/r9）。
fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> u64 {
    let ret: u64;
    // SAFETY: syscall 指令为 x86_64 标准接口；rcx/r11 被 CPU 覆盖。
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") nr => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            in("r9") a6,
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

/// 文件写读往返（M4-切片1/4 验收）：创建 → 写入 → 读回验证。
fn file_roundtrip(p: &[u8]) {
    let fd = syscall3(SYS_OPEN, p.as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
    if (fd as i64) < 0 {
        print("fstest: create failed rc=");
        print_u64(fd);
        print("\n");
        return;
    }
    let msg = b"hello from ramfs\n";
    syscall3(SYS_WRITE, fd, msg.as_ptr() as u64, msg.len() as u64);
    syscall3(SYS_CLOSE, fd, 0, 0);
    let fd2 = syscall3(SYS_OPEN, p.as_ptr() as u64, 0, 0);
    let mut buf = [0u8; 64];
    let n = syscall3(SYS_READ, fd2, buf.as_mut_ptr() as u64, 64);
    print("fstest: read ");
    print_u64(n);
    print("B: ");
    print(unsafe { core::str::from_utf8_unchecked(&buf[..n as usize]) });
    syscall3(SYS_CLOSE, fd2, 0, 0);
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
            print("commands: help | ls [dir] | cat <f> | echo <text> | mkdir <d> | rm <f> | rmdir <d> | mount <d> | stat <f> | version | fdtest | fstest [path] | dtest | udptest | tcptest | httptest | forktest | exit\n");
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
            file_roundtrip(b"/etc/motd\0");
        }
        _ if cmd.starts_with(b"fstest ") => {
            let p = path_arg(cmd, 7);
            file_roundtrip(&p);
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
        b"udptest" => {
            // M5-切片3：UDP socket 全链路验证（双 hostfwd 规则自环回）。
            // 说明：QEMU/Windows 的 slirp UDP hostfwd 只转发 guest 发起方向的包，
            // 宿主主动发 UDP 无法进 guest；故用两条规则各回环一包验证
            // create/bind/sendto → virtio TX → slirp(宿主侧) → virtio RX → demux → recvfrom。
            let fd = syscall3(SYS_SOCKET, 2, 2, 0); // AF_INET, SOCK_DGRAM
            if (fd as i64) < 0 {
                print("udptest: socket failed rc=");
                print_u64(fd);
                print("\n");
            } else {
                // bind 19999（sockaddr_in：family@0, port@2 BE）
                let mut sa = [0u8; 16];
                sa[0..2].copy_from_slice(&2u16.to_le_bytes());
                sa[2..4].copy_from_slice(&19999u16.to_be_bytes());
                syscall3(SYS_BIND, fd, sa.as_ptr() as u64, 16);
                // 1) sendto 10.0.2.2:12345（规则 12345→19999 回环，20B）
                let mut da = [0u8; 16];
                da[0..2].copy_from_slice(&2u16.to_le_bytes());
                da[2..4].copy_from_slice(&12345u16.to_be_bytes());
                da[4..8].copy_from_slice(&[10, 0, 2, 2]);
                let msg1 = b"hello udp from novos";
                let rc = syscall6(
                    SYS_SENDTO,
                    fd,
                    msg1.as_ptr() as u64,
                    msg1.len() as u64,
                    0,
                    da.as_ptr() as u64,
                    16,
                );
                print("udptest: sent rc=");
                print_u64(rc);
                print("\n");
                // 2) sendto 10.0.2.2:12344（规则 12344→19999 回环，15B）
                da[2..4].copy_from_slice(&12344u16.to_be_bytes());
                let msg2 = b"pong from novos";
                let rc2 = syscall6(
                    SYS_SENDTO,
                    fd,
                    msg2.as_ptr() as u64,
                    msg2.len() as u64,
                    0,
                    da.as_ptr() as u64,
                    16,
                );
                print("udptest: sent2 rc=");
                print_u64(rc2);
                print("\n");
                // recvfrom 收 2 条（两个 hostfwd 规则的回环包）
                let mut got = 0u32;
                let mut tries = 0u32;
                while got < 2 && tries < 150 {
                    let mut rb = [0u8; 128];
                    let n = syscall6(SYS_RECVFROM, fd, rb.as_mut_ptr() as u64, 128, 0, 0, 0);
                    if (n as i64) > 0 {
                        print("udptest: recv ");
                        print_u64(n);
                        print("B: ");
                        print(unsafe { core::str::from_utf8_unchecked(&rb[..n as usize]) });
                        print("\n");
                        got += 1;
                    }
                    tries += 1;
                    // 空转延时（QEMU 下约数十 ms）
                    let mut spin = 0u32;
                    while spin < 4_000_000 {
                        spin += 1;
                    }
                }
                if got < 2 {
                    print("udptest: recv timeout (got ");
                    print_u64(got.into());
                    print(")\n");
                }
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
        }
        b"tcptest" => {
            // M5-切片4：TCP echo 服务——hostfwd(tcp:20000) 下宿主连接后
            // 收数据原样回发。验证三次握手 / accept / recv / send / 关闭。
            let fd = syscall3(SYS_SOCKET, 2, 1, 0); // AF_INET, SOCK_STREAM
            if (fd as i64) < 0 {
                print("tcptest: socket failed\n");
            } else {
                let mut sa = [0u8; 16];
                sa[0..2].copy_from_slice(&2u16.to_le_bytes());
                sa[2..4].copy_from_slice(&20000u16.to_be_bytes());
                syscall3(SYS_BIND, fd, sa.as_ptr() as u64, 16);
                syscall3(SYS_LISTEN, fd, 4, 0);
                print("tcptest: listening on 20000\n");
                // accept：非阻塞自旋等宿主连接（握手完成即 Established，无超时）
                let mut afd: u64 = 0;
                while afd == 0 {
                    afd = syscall3(SYS_ACCEPT, fd, 0, 0);
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("tcptest: accepted fd=");
                print_u64(afd);
                print("\n");
                // recv：等宿主数据（无超时）
                let mut rb = [0u8; 128];
                let mut got: u64 = 0;
                while got == 0 {
                    got = syscall6(SYS_RECVFROM, afd, rb.as_mut_ptr() as u64, 128, 0, 0, 0);
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("tcptest: recv ");
                print_u64(got);
                print("B: ");
                print(unsafe { core::str::from_utf8_unchecked(&rb[..got as usize]) });
                print("\n");
                // echo 回发
                let n = syscall6(SYS_SENDTO, afd, rb.as_mut_ptr() as u64, got, 0, 0, 0);
                print("tcptest: echoed ");
                print_u64(n);
                print("\n");
                syscall3(SYS_CLOSE, afd, 0, 0);
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
        }
        b"httptest" => {
            // M5-切片5：HTTP 服务——hostfwd(tcp:80) 下宿主 GET 请求，
            // epoll 等连接可读后读请求并回 HTTP 200。
            let fd = syscall3(SYS_SOCKET, 2, 1, 0); // AF_INET, SOCK_STREAM
            if (fd as i64) < 0 {
                print("httptest: socket failed\n");
            } else {
                let mut sa = [0u8; 16];
                sa[0..2].copy_from_slice(&2u16.to_le_bytes());
                sa[2..4].copy_from_slice(&80u16.to_be_bytes());
                syscall3(SYS_BIND, fd, sa.as_ptr() as u64, 16);
                syscall3(SYS_LISTEN, fd, 4, 0);
                print("httptest: listening on 80\n");
                // accept：非阻塞自旋等宿主连接
                let mut afd: u64 = 0;
                while afd == 0 {
                    afd = syscall3(SYS_ACCEPT, fd, 0, 0);
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("httptest: accepted fd=");
                print_u64(afd);
                print("\n");
                // epoll：注册 afd 的 EPOLLIN，轮询等数据
                let epfd = syscall3(SYS_EPOLL_CREATE, 0, 0, 0);
                let mut ev = [0u8; 12];
                ev[0..4].copy_from_slice(&1u32.to_le_bytes()); // EPOLLIN=1
                syscall5(SYS_EPOLL_CTL, epfd, 1, afd, ev.as_ptr() as u64, 0); // ADD
                print("httptest: epoll waiting\n");
                let mut evout = [0u8; 12];
                let mut woke: u64 = 0;
                while woke == 0 {
                    woke = syscall5(SYS_EPOLL_WAIT, epfd, evout.as_mut_ptr() as u64, 1, 0, 0);
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("httptest: epoll wake\n");
                // 读请求
                let mut rb = [0u8; 256];
                let mut rn: u64 = 0;
                let mut rtries = 0u32;
                while rn == 0 {
                    rn = syscall6(SYS_RECVFROM, afd, rb.as_mut_ptr() as u64, 256, 0, 0, 0);
                    rtries += 1;
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                    if rtries > 200 {
                        break;
                    }
                }
                print("httptest: got request ");
                print_u64(rn);
                print("B\n");
                // 响应 HTTP 200（body 25B "<h1>Novos-OS HTTP OK</h1>"）
                let resp =
                    b"HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nContent-Length: 25\r\nConnection: close\r\n\r\n<h1>Novos-OS HTTP OK</h1>";
                let sn = syscall6(SYS_SENDTO, afd, resp.as_ptr() as u64, resp.len() as u64, 0, 0, 0);
                print("httptest: served ");
                print_u64(sn);
                print("B\n");
                syscall3(SYS_CLOSE, afd, 0, 0);
                syscall3(SYS_CLOSE, epfd, 0, 0);
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
        }
        b"forktest" => {
            // M6-切片1：用户态 fork + pid namespace。
            // 父打印自身 pid（根 ns = 1）；子 A 打印 getpid（根 ns 顺序号 2）；
            // clone(CLONE_NEWPID) 的子 B 进入新 pid ns，getpid = 1。
            print("forktest: parent getpid=");
            print_u64(syscall3(SYS_GETPID, 0, 0, 0));
            print("\n");
            // 1) fork：子返回 0，父返回子任务 id
            let ra = syscall3(SYS_FORK, 0, 0, 0);
            if (ra as i64) < 0 {
                print("forktest: fork failed\n");
            } else if ra == 0 {
                // 子 A：打印 ns pid 后退出
                print("forktest: child A getpid=");
                print_u64(syscall3(SYS_GETPID, 0, 0, 0));
                print("\n");
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                print("forktest: fork -> child id=");
                print_u64(ra);
                print("\n");
                // 父：waitpid 自旋回收
                let mut wa = 0u64;
                let mut n = 0u32;
                while wa == 0 && n < 200000 {
                    wa = syscall3(SYS_WAITPID, ra, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                print("forktest: waitpid A reaped=");
                print_u64(wa);
                print("\n");
            }
            // 2) clone(CLONE_NEWPID)：子 B 进入新 pid ns
            if (ra as i64) < 0 {
                // fork 失败则不继续
            } else {
                let rb = syscall3(SYS_CLONE, 0x2000_0000, 0, 0); // CLONE_NEWPID
                if rb == 0 {
                    print("forktest: child B getpid=");
                    print_u64(syscall3(SYS_GETPID, 0, 0, 0));
                    print(" (new pid ns)\n");
                    syscall3(SYS_EXIT, 0, 0, 0);
                } else {
                    print("forktest: clone NEWPID -> child id=");
                    print_u64(rb);
                    print("\n");
                    let mut wb = 0u64;
                    let mut n2 = 0u32;
                    while wb == 0 && n2 < 200000 {
                        wb = syscall3(SYS_WAITPID, rb, 0, 0);
                        n2 += 1;
                        let mut s = 0u32;
                        while s < 50000 {
                            s += 1;
                        }
                    }
                    print("forktest: waitpid B reaped=");
                    print_u64(wb);
                    print("\n");
                }
            }
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
        _ if cmd.starts_with(b"mount ") => {
            // mount tmpfs 到目标目录（M4-切片4）
            let p = path_arg(cmd, 6);
            let rc = syscall5(SYS_MOUNT, b"tmpfs\0".as_ptr() as u64, p.as_ptr() as u64, 0, 0, 0);
            if rc != 0 {
                print("mount: rc=");
                print_u64(rc);
                print("\n");
            }
        }
        _ if cmd.starts_with(b"stat ") => {
            let p = path_arg(cmd, 5);
            let mut buf = [0u8; 144];
            let rc = syscall3(SYS_STAT, p.as_ptr() as u64, buf.as_mut_ptr() as u64, 0);
            if rc != 0 {
                print("stat: rc=");
                print_u64(rc);
                print("\n");
            } else {
                let mode = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
                let size = i64::from_le_bytes([
                    buf[48], buf[49], buf[50], buf[51], buf[52], buf[53], buf[54], buf[55],
                ]);
                print("stat: mode=");
                print_u64(mode as u64);
                print(" size=");
                print_u64(size as u64);
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
