//! 山水观心操作系统用户态 init/shell（M3 切片4）。
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
const SYS_UNAME: u64 = 63;
const SYS_SETHOSTNAME: u64 = 170;
const SYS_CGROUP_STAT: u64 = 500;
const SYS_MOUNT: u64 = 165;
const SYS_GETDENTS64: u64 = 217;
const SYS_GETCWD: u64 = 79;
const SYS_CHDIR: u64 = 80;
const SYS_FUTEX: u64 = 202;
const SYS_ARCH_PRCTL: u64 = 158;
const SYS_RT_SIGACTION: u64 = 13;   // M13-06：注册信号 handler
const SYS_RT_SIGRETURN: u64 = 15;   // M13-06：从信号帧恢复
const SYS_NAT_ADD: u64 = 501;
const SYS_CT_STAT: u64 = 502;
const SYS_FW_ADD: u64 = 503;
const SYS_FW_DEL: u64 = 504;
const SYS_FW_STAT: u64 = 505;
const SYS_BLK_RW: u64 = 506;
const SYS_BLKFS: u64 = 507;

// open flags（Linux O_*）
const O_CREAT: u64 = 0o100;
const O_TRUNC: u64 = 0o1000;

// M11-切片2：TLS 测试缓冲——arch_prctl 把 FS base 指到这些静态缓冲，
// 用户态经 `%fs` 段寻址访问（验证 FS base MSR 生效 + 上下文切换恢复）。
static mut TLS_BUF_A: [u8; 32] = [0; 32];
static mut TLS_BUF_B: [u8; 32] = [0; 32];

// M11-切片3：clone 测试（CLONE_SETTLS + CLONE_CHILD_CLEARTID）资源
static mut CLONE_STACK: [u8; 8192] = [0; 8192];
static mut CLONE_TID: u32 = 0;
static mut CLONE_TLS: [u8; 32] = [0; 32];
static mut CLONE_GO: u32 = 0;

// M11-切片4：futex REQUEUE + 超时测试资源
static mut RQ_CV: u32 = 0; // 条件变量 futex（双等待者阻塞点）
static mut RQ_MUTEX: u32 = 0; // requeue 目标 futex
static mut RQ_GO_A: u32 = 0; // A 放行
static mut RQ_GO_B: u32 = 0; // B 放行
static mut RQ_RDY_B: u32 = 0; // B 已阻塞到 cv 的握手标志
static mut RQ_TM: u32 = 0; // 超时测试
static mut RQ_WOKE: u32 = 0; // 被唤醒计数

// M13-01：/proc/self/maps 测试缓冲
static mut MAP_BUF: [u8; 4096] = [0; 4096];

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

/// 字节子串查找（maptest 用）。
fn scan_bytes(hay: &[u8], needle: &[u8]) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    let mut i = 0usize;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            return true;
        }
        i += 1;
    }
    false
}

// ---- M13-06：信号测试（sigtest）----
// 信号帧 ABI 与 kernel/src/signal.rs 严格对齐（SigFrame 布局）。
const SIGSEGV: u64 = 11;
const SA_SIGINFO: u64 = 0x4;

#[repr(C)]
struct SigAction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

/// 与内核 ExceptionFrame 对齐（rt_sigreturn 恢复 / handler 改 rip 跳恢复点）。
#[repr(C)]
struct SavedRegs {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    vec: u64,
    err: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

/// ucontext（内核 SigFrame +0x08，saved 在 +0x38）。
#[repr(C)]
struct UContext {
    flags: u64,
    link: u64,
    stack_sp: u64,
    stack_flags: u64,
    stack_size: u64,
    sigmask: u64,
    mcontext: SavedRegs,
}

/// siginfo（内核 SigFrame +0xC0）。
#[repr(C)]
struct SigInfo {
    signo: u64,
    errno: u64,
    code: u64,
    addr: u64,
    pid: u64,
    uid: u64,
}

static mut HANDLED_SIGNOS: u64 = 0;
static mut HANDLED_ADDR: u64 = 0;
/// 信号已触发（首次运行置位；handler 恢复后据此跳过触发段）。
static mut SIG_ACTIVE: bool = false;
/// setjmp 式恢复点（jmp_set 保存）。
static mut JB_RSP: u64 = 0;
static mut JB_RBP: u64 = 0;
static mut JB_RET: u64 = 0;

/// 保存当前 rsp/rbp/返回地址（handler 经 rt_sigreturn 恢复后跳到保存点继续）。
#[inline(never)]
fn jmp_set() {
    // SAFETY: 单核测试环境；仅读 rsp/rbp/[rsp]，不改栈。
    unsafe {
        let rsp: u64;
        let rbp: u64;
        let ret: u64;
        core::arch::asm!(
            "mov {0}, rsp",
            "mov {1}, rbp",
            "mov {2}, [rsp]",
            out(reg) rsp,
            out(reg) rbp,
            out(reg) ret,
            options(nostack)
        );
        JB_RSP = rsp;
        JB_RBP = rbp;
        JB_RET = ret;
    }
}

/// SIGSEGV handler（内核按 C ABI 调用：rdi=signo, rsi=&siginfo, rdx=&ucontext）。
/// 以 `rt_sigreturn` 收尾（不返回）；把 mcontext 恢复为 jmp_set 保存点，
/// 跳过触发指令并以保存的干净栈继续 sigtest。
extern "C" fn segv_handler(signo: i32, info: *mut SigInfo, uctx: *mut UContext) -> ! {
    // SAFETY: 内核保证三参数有效；单核测试环境。
    unsafe {
        HANDLED_SIGNOS = signo as u64;
        HANDLED_ADDR = (*info).addr;
        (*uctx).mcontext.rsp = JB_RSP;
        (*uctx).mcontext.rbp = JB_RBP;
        (*uctx).mcontext.rip = JB_RET;
    }
    syscall3(SYS_RT_SIGRETURN, 0, 0, 0);
    loop {}
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

/// 打印当前 uts ns 的 hostname（uname 的 nodename 字段，偏移 65）。
fn print_hostname() {
    let mut u = [0u8; 390];
    syscall3(SYS_UNAME, u.as_mut_ptr() as u64, 0, 0);
    let mut i = 65usize;
    while i < 130 && u[i] != 0 {
        i += 1;
    }
    // SAFETY: u[65..i] 为 hostname ASCII。
    print(unsafe { core::str::from_utf8_unchecked(&u[65..i]) });
}

/// 打印当前 cgroup 统计 { pids, mem }。
fn print_cg(label: &str) {
    let mut st = [0u8; 16];
    syscall3(SYS_CGROUP_STAT, st.as_mut_ptr() as u64, 0, 0);
    let pids = u64::from_le_bytes([st[0], st[1], st[2], st[3], st[4], st[5], st[6], st[7]]);
    let mem = u64::from_le_bytes([st[8], st[9], st[10], st[11], st[12], st[13], st[14], st[15]]);
    print(label);
    print("pids=");
    print_u64(pids);
    print(" mem=");
    print_u64(mem);
    print("\n");
}

/// 内建命令执行。
fn exec(cmd: &[u8]) {
    match cmd {
        [] => {}
        b"help" => {
            print("commands: help | ls [dir] | cat <f> | echo <text> | mkdir <d> | rm <f> | rmdir <d> | mount <d> | stat <f> | cd <d> | pwd | version | fdtest | fstest [path] | dtest | udptest | tcptest | httptest | forktest | utstest | cgtest | ovltest | whtest | shanshui-guanxin | natdemo | fwtest | proctest | healthtest | blktest | ext4test | futtest | tlstest | clonetest | reqtest | maptest | statustest | sigtest | exit\n");
        }
        b"version" => {
            print("Shanshui-guanxin userspace init v0.3.0 (M3)\n");
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
                let msg1 = b"hello udp from shanshui-guanxin";
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
                let msg2 = b"pong from shanshui-guanxin";
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
                // 响应 HTTP 200（body 25B "<h1>Shanshui-guanxin HTTP OK</h1>"）
                let resp =
                    b"HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nContent-Length: 25\r\nConnection: close\r\n\r\n<h1>Shanshui-guanxin HTTP OK</h1>";
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
        b"utstest" => {
            // M6-切片2：uts namespace——CLONE_NEWUTS 子进程改 hostname 不影响父。
            print("utstest: parent hostname=");
            print_hostname();
            print("\n");
            let r = syscall3(SYS_CLONE, 0x0400_0000, 0, 0); // CLONE_NEWUTS
            if r == 0 {
                // 子：设置自己的 hostname 并打印
                let hn = b"childns";
                syscall3(SYS_SETHOSTNAME, hn.as_ptr() as u64, hn.len() as u64, 0);
                print("utstest: child hostname=");
                print_hostname();
                print("\n");
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                // 父：等子退出后打印自身 hostname（应仍是 shanshui-guanxin）
                let mut w = 0u64;
                let mut n = 0u32;
                while w == 0 && n < 200000 {
                    w = syscall3(SYS_WAITPID, r, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                print("utstest: parent hostname after=");
                print_hostname();
                print("\n");
            }
        }
        b"cgtest" => {
            // M6-切片3：cgroup pids + 内存记账——fork 子进程后统计配对无泄漏。
            print_cg("cgtest: root ");
            let r = syscall3(SYS_FORK, 0, 0, 0);
            if r == 0 {
                // 子：pids +1，mem +64KB（子内核栈记账）
                print_cg("cgtest: child ");
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                // 父：等子退出回收，统计应回到基线（无泄漏）
                let mut w = 0u64;
                let mut n = 0u32;
                while w == 0 && n < 200000 {
                    w = syscall3(SYS_WAITPID, r, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                print_cg("cgtest: after reap ");
            }
        }
        b"ovltest" => {
            // M7-切片1：OverlayFS——只读 lower 上叠可写 upper，写后下层不变。
            // 1) 准备 lower：/lower/base.txt = "lower-data"
            syscall3(SYS_MKDIR, b"/lower\0".as_ptr() as u64, 0o755, 0);
            syscall3(SYS_MKDIR, b"/mnt/ovl\0".as_ptr() as u64, 0o755, 0);
            let fd = syscall3(SYS_OPEN, b"/lower/base.txt\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
            let msg = b"lower-data";
            syscall3(SYS_WRITE, fd, msg.as_ptr() as u64, msg.len() as u64);
            syscall3(SYS_CLOSE, fd, 0, 0);
            // 2) mount overlay：lower=/lower, upper=新建
            let rc = syscall5(SYS_MOUNT, b"/lower\0".as_ptr() as u64, b"/mnt/ovl\0".as_ptr() as u64, b"overlay\0".as_ptr() as u64, 0, 0);
            if rc != 0 {
                print("ovltest: mount rc=");
                print_u64(rc);
                print("\n");
            } else {
                print("ovltest: overlay mounted\n");
                // 3) 经 overlay 读 lower 文件
                let fd2 = syscall3(SYS_OPEN, b"/mnt/ovl/base.txt\0".as_ptr() as u64, 0, 0);
                if (fd2 as i64) >= 0 {
                    let mut rb = [0u8; 32];
                    let n = syscall3(SYS_READ, fd2, rb.as_mut_ptr() as u64, 32);
                    syscall3(SYS_CLOSE, fd2, 0, 0);
                    print("ovltest: overlay read: ");
                    print(unsafe { core::str::from_utf8_unchecked(&rb[..n as usize]) });
                    print("\n");
                } else {
                    print("ovltest: overlay open failed\n");
                }
                // 4) copy-up 写：打开 overlay 视图写 "overlay-write"
                let fd3 = syscall3(SYS_OPEN, b"/mnt/ovl/base.txt\0".as_ptr() as u64, 1 | O_TRUNC, 0);
                let m2 = b"overlay-write";
                let w = syscall3(SYS_WRITE, fd3, m2.as_ptr() as u64, m2.len() as u64);
                syscall3(SYS_CLOSE, fd3, 0, 0);
                print("ovltest: copy-up write ");
                print_u64(w);
                print("B\n");
                // 5) 读回 overlay 视图
                let fd4 = syscall3(SYS_OPEN, b"/mnt/ovl/base.txt\0".as_ptr() as u64, 0, 0);
                let mut rb2 = [0u8; 32];
                let n2 = syscall3(SYS_READ, fd4, rb2.as_mut_ptr() as u64, 32);
                syscall3(SYS_CLOSE, fd4, 0, 0);
                print("ovltest: overlay read: ");
                print(unsafe { core::str::from_utf8_unchecked(&rb2[..n2 as usize]) });
                print("\n");
                // 6) lower 不变
                let fd5 = syscall3(SYS_OPEN, b"/lower/base.txt\0".as_ptr() as u64, 0, 0);
                let mut rb3 = [0u8; 32];
                let n3 = syscall3(SYS_READ, fd5, rb3.as_mut_ptr() as u64, 32);
                syscall3(SYS_CLOSE, fd5, 0, 0);
                print("ovltest: lower unchanged: ");
                print(unsafe { core::str::from_utf8_unchecked(&rb3[..n3 as usize]) });
                print("\n");
                // 7) upper 新建文件（lower 无对应）
                let fd6 = syscall3(SYS_OPEN, b"/mnt/ovl/new.txt\0".as_ptr() as u64, O_CREAT | 1, 0o644);
                if (fd6 as i64) >= 0 {
                    let m3 = b"upper-new";
                    syscall3(SYS_WRITE, fd6, m3.as_ptr() as u64, m3.len() as u64);
                    syscall3(SYS_CLOSE, fd6, 0, 0);
                    print("ovltest: upper new file ok\n");
                } else {
                    print("ovltest: upper new failed\n");
                }
            }
        }
        b"whtest" => {
            // M7-切片2：whiteout——删除 lower 文件后合并视图"已删除"，lower 保持只读；
            //          同时演示容器日志目录默认 tmpfs（日志不触发 overlay copy-up）。
            // 1) 独立 lower：/wlower/{del.txt, keep.txt}
            syscall3(SYS_MKDIR, b"/wlower\0".as_ptr() as u64, 0o755, 0);
            syscall3(SYS_MKDIR, b"/mnt/wht\0".as_ptr() as u64, 0o755, 0);
            let fd = syscall3(SYS_OPEN, b"/wlower/del.txt\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
            let d1 = b"to-delete";
            syscall3(SYS_WRITE, fd, d1.as_ptr() as u64, d1.len() as u64);
            syscall3(SYS_CLOSE, fd, 0, 0);
            let fd = syscall3(SYS_OPEN, b"/wlower/keep.txt\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
            let d2 = b"keep-data";
            syscall3(SYS_WRITE, fd, d2.as_ptr() as u64, d2.len() as u64);
            syscall3(SYS_CLOSE, fd, 0, 0);
            // 2) mount overlay：lower=/wlower, upper=新建
            let rc = syscall5(SYS_MOUNT, b"/wlower\0".as_ptr() as u64, b"/mnt/wht\0".as_ptr() as u64, b"overlay\0".as_ptr() as u64, 0, 0);
            if rc != 0 {
                print("whtest: mount rc=");
                print_u64(rc);
                print("\n");
            } else {
                // 3) 经 overlay 读到 del.txt
                let fd2 = syscall3(SYS_OPEN, b"/mnt/wht/del.txt\0".as_ptr() as u64, 0, 0);
                if (fd2 as i64) >= 0 {
                    let mut rb = [0u8; 32];
                    let n = syscall3(SYS_READ, fd2, rb.as_mut_ptr() as u64, 32);
                    syscall3(SYS_CLOSE, fd2, 0, 0);
                    print("whtest: overlay read: ");
                    print(unsafe { core::str::from_utf8_unchecked(&rb[..n as usize]) });
                    print("\n");
                }
                // 4) 删除 lower 文件 → 触发 whiteout
                let rc2 = syscall3(SYS_UNLINK, b"/mnt/wht/del.txt\0".as_ptr() as u64, 0, 0);
                print("whtest: unlink rc=");
                print_u64(rc2);
                print("\n");
                // 5) 再开 → ENOENT（whiteout 生效）
                let fd3 = syscall3(SYS_OPEN, b"/mnt/wht/del.txt\0".as_ptr() as u64, 0, 0);
                if (fd3 as i64) < 0 {
                    print("whtest: deleted via whiteout\n");
                } else {
                    syscall3(SYS_CLOSE, fd3, 0, 0);
                    print("whtest: whiteout FAILED (still visible)\n");
                }
                // 6) lower 直读不受影响（只读保证）
                let fd4 = syscall3(SYS_OPEN, b"/wlower/del.txt\0".as_ptr() as u64, 0, 0);
                if (fd4 as i64) >= 0 {
                    let mut rb2 = [0u8; 32];
                    let n2 = syscall3(SYS_READ, fd4, rb2.as_mut_ptr() as u64, 32);
                    syscall3(SYS_CLOSE, fd4, 0, 0);
                    print("whtest: lower intact: ");
                    print(unsafe { core::str::from_utf8_unchecked(&rb2[..n2 as usize]) });
                    print("\n");
                }
                // 7) 合并视图枚举：del.txt 消失、keep.txt 仍在、无 .wh.* 标记
                let fd5 = syscall3(SYS_OPEN, b"/mnt/wht\0".as_ptr() as u64, 0, 0);
                let mut found_keep = false;
                let mut found_del = false;
                let mut found_wh = false;
                loop {
                    let mut buf = [0u8; 512];
                    let n = syscall3(SYS_GETDENTS64, fd5, buf.as_mut_ptr() as u64, 512);
                    if n == 0 {
                        break;
                    }
                    let mut off = 0usize;
                    while off + 19 <= n as usize {
                        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                        let mut nl = 0usize;
                        while off + 19 + nl < buf.len() && buf[off + 19 + nl] != 0 {
                            nl += 1;
                        }
                        if nl > 0 {
                            // SAFETY: 目录项名称为 ASCII。
                            let nm = unsafe {
                                core::str::from_utf8_unchecked(&buf[off + 19..off + 19 + nl])
                            };
                            if nm == "keep.txt" {
                                found_keep = true;
                            }
                            if nm == "del.txt" {
                                found_del = true;
                            }
                            if nm.starts_with(".wh.") {
                                found_wh = true;
                            }
                        }
                        off += reclen;
                    }
                }
                syscall3(SYS_CLOSE, fd5, 0, 0);
                if found_keep && !found_del && !found_wh {
                    print("whtest: merged listing clean\n");
                } else {
                    print("whtest: listing keep=");
                    print_u64(found_keep as u64);
                    print(" del=");
                    print_u64(found_del as u64);
                    print(" wh=");
                    print_u64(found_wh as u64);
                    print("\n");
                }
                // 8) 重建同路径文件（覆盖 whiteout 后新建）
                let fd6 = syscall3(SYS_OPEN, b"/mnt/wht/del.txt\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
                if (fd6 as i64) >= 0 {
                    let m3 = b"reborn";
                    syscall3(SYS_WRITE, fd6, m3.as_ptr() as u64, m3.len() as u64);
                    syscall3(SYS_CLOSE, fd6, 0, 0);
                    let fd7 = syscall3(SYS_OPEN, b"/mnt/wht/del.txt\0".as_ptr() as u64, 0, 0);
                    let mut rb3 = [0u8; 32];
                    let n3 = syscall3(SYS_READ, fd7, rb3.as_mut_ptr() as u64, 32);
                    syscall3(SYS_CLOSE, fd7, 0, 0);
                    print("whtest: recreate: ");
                    print(unsafe { core::str::from_utf8_unchecked(&rb3[..n3 as usize]) });
                    print("\n");
                } else {
                    print("whtest: recreate failed\n");
                }
            }
            // 9) 容器日志 tmpfs：/logs 挂 tmpfs，写读日志文件
            syscall3(SYS_MKDIR, b"/logs\0".as_ptr() as u64, 0o755, 0);
            let rc3 = syscall5(SYS_MOUNT, b"tmpfs\0".as_ptr() as u64, b"/logs\0".as_ptr() as u64, b"tmpfs\0".as_ptr() as u64, 0, 0);
            if rc3 != 0 {
                print("whtest: log mount rc=");
                print_u64(rc3);
                print("\n");
            } else {
                let fd8 = syscall3(SYS_OPEN, b"/logs/container.log\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
                let lg = b"log-line-1\n";
                syscall3(SYS_WRITE, fd8, lg.as_ptr() as u64, lg.len() as u64);
                syscall3(SYS_CLOSE, fd8, 0, 0);
                let fd9 = syscall3(SYS_OPEN, b"/logs/container.log\0".as_ptr() as u64, 0, 0);
                let mut rb4 = [0u8; 32];
                let n4 = syscall3(SYS_READ, fd9, rb4.as_mut_ptr() as u64, 32);
                syscall3(SYS_CLOSE, fd9, 0, 0);
                print("whtest: log tmpfs read: ");
                print(unsafe { core::str::from_utf8_unchecked(&rb4[..n4 as usize]) });
                print("\n");
            }
        }
        b"shanshui-guanxin" => {
            // M8-切片1：容器运行时骨架——overlay rootfs 组装 + ns/cgroup 隔离 +
            // 生命周期回收（类 runC 流程：准备镜像 → 挂 rootfs → clone 进 ns → 执行 → 回收）。
            // 1) 准备镜像 lower：/img/app.txt（宿主侧"镜像层"）
            syscall3(SYS_MKDIR, b"/img\0".as_ptr() as u64, 0o755, 0);
            syscall3(SYS_MKDIR, b"/containers\0".as_ptr() as u64, 0o755, 0);
            syscall3(SYS_MKDIR, b"/containers/c0\0".as_ptr() as u64, 0o755, 0);
            syscall3(SYS_MKDIR, b"/containers/c0/rootfs\0".as_ptr() as u64, 0o755, 0);
            let fd = syscall3(SYS_OPEN, b"/img/app.txt\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
            let img = b"app-data";
            syscall3(SYS_WRITE, fd, img.as_ptr() as u64, img.len() as u64);
            syscall3(SYS_CLOSE, fd, 0, 0);
            // 2) overlay rootfs：lower=/img, upper=新建, target=/containers/c0/rootfs
            let rc = syscall5(SYS_MOUNT, b"/img\0".as_ptr() as u64, b"/containers/c0/rootfs\0".as_ptr() as u64, b"overlay\0".as_ptr() as u64, 0, 0);
            if rc != 0 {
                print("shanshui-guanxin run: mount rc=");
                print_u64(rc);
                print("\n");
            } else {
                print("shanshui-guanxin run: rootfs mounted\n");
                // 3) 容器进程：CLONE_NEWPID | CLONE_NEWUTS（pid/uts 隔离）
                let r = syscall3(SYS_CLONE, 0x2000_0000 | 0x0400_0000, 0, 0);
                if r == 0 {
                    // 容器 init（子进程）：新 pid ns 内 pid=1，独立 hostname
                    syscall3(SYS_SETHOSTNAME, b"c0\0".as_ptr() as u64, 2, 0);
                    print("shanshui-guanxin run: container init pid=");
                    print_u64(syscall3(SYS_GETPID, 0, 0, 0));
                    print(" host=");
                    print_hostname();
                    print("\n");
                    // 4) chdir 进容器 rootfs（cwd 隔离，后续相对路径落在容器内）
                    let cr = syscall3(SYS_CHDIR, b"/containers/c0/rootfs\0".as_ptr() as u64, 0, 0);
                    let mut cwdb = [0u8; 64];
                    let cn = syscall3(SYS_GETCWD, cwdb.as_mut_ptr() as u64, 64, 0);
                    print("shanshui-guanxin run: chdir rc=");
                    print_u64(cr);
                    print(" cwd=");
                    print(unsafe { core::str::from_utf8_unchecked(&cwdb[..cn as usize]) });
                    print("\n");
                    // 5) 读镜像层文件（经 rootfs 合并视图，相对路径）
                    let fd2 = syscall3(SYS_OPEN, b"app.txt\0".as_ptr() as u64, 0, 0);
                    if (fd2 as i64) >= 0 {
                        let mut rb = [0u8; 32];
                        let n = syscall3(SYS_READ, fd2, rb.as_mut_ptr() as u64, 32);
                        syscall3(SYS_CLOSE, fd2, 0, 0);
                        print("shanshui-guanxin run: rootfs read: ");
                        print(unsafe { core::str::from_utf8_unchecked(&rb[..n as usize]) });
                        print("\n");
                    } else {
                        print("shanshui-guanxin run: rootfs read FAILED\n");
                    }
                    // 6) 容器写：/var/log 目录 + 日志文件（进 upper 层，镜像层不变）
                    syscall3(SYS_MKDIR, b"var\0".as_ptr() as u64, 0o755, 0);
                    syscall3(SYS_MKDIR, b"var/log\0".as_ptr() as u64, 0o755, 0);
                    let fd3 = syscall3(SYS_OPEN, b"var/log/container.log\0".as_ptr() as u64, O_CREAT | 1 | O_TRUNC, 0o644);
                    let log = b"c0-booted\n";
                    syscall3(SYS_WRITE, fd3, log.as_ptr() as u64, log.len() as u64);
                    syscall3(SYS_CLOSE, fd3, 0, 0);
                    let fd4 = syscall3(SYS_OPEN, b"var/log/container.log\0".as_ptr() as u64, 0, 0);
                    let mut rb2 = [0u8; 32];
                    let n2 = syscall3(SYS_READ, fd4, rb2.as_mut_ptr() as u64, 32);
                    syscall3(SYS_CLOSE, fd4, 0, 0);
                    print("shanshui-guanxin run: container log: ");
                    print(unsafe { core::str::from_utf8_unchecked(&rb2[..n2 as usize]) });
                    print("\n");
                    // 7) cgroup 记账（容器进程计入根 cgroup，pids+1 / mem+64KB）
                    print_cg("shanshui-guanxin run: cg ");
                    syscall3(SYS_EXIT, 0, 0, 0);
                } else {
                    // 宿主（父）：等容器 init 退出并回收
                    let mut w = 0u64;
                    let mut n = 0u32;
                    while w == 0 && n < 300000 {
                        w = syscall3(SYS_WAITPID, r, 0, 0);
                        n += 1;
                        let mut s = 0u32;
                        while s < 50000 {
                            s += 1;
                        }
                    }
                    print("shanshui-guanxin run: reaped=");
                    print_u64(w);
                    print("\n");
                    // 8) 回收后 cgroup 回到基线（无泄漏）
                    print_cg("shanshui-guanxin run: after reap ");
                    // 镜像层不被容器写污染
                    let fd5 = syscall3(SYS_OPEN, b"/img/app.txt\0".as_ptr() as u64, 0, 0);
                    let mut rb3 = [0u8; 32];
                    let n3 = syscall3(SYS_READ, fd5, rb3.as_mut_ptr() as u64, 32);
                    syscall3(SYS_CLOSE, fd5, 0, 0);
                    print("shanshui-guanxin run: image intact: ");
                    print(unsafe { core::str::from_utf8_unchecked(&rb3[..n3 as usize]) });
                    print("\n");
                    print("shanshui-guanxin run: container exited\n");
                }
            }
        }
        b"natdemo" => {
            // M8-切片2：网关 DNAT 端口映射——容器服务监听 8080，网关把对外 20001
            // 映射进来（conntrack 会话跟踪 + 回包反向还原）。
            let fd = syscall3(SYS_SOCKET, 2, 1, 0); // AF_INET, SOCK_STREAM
            if (fd as i64) < 0 {
                print("natdemo: socket failed\n");
            } else {
                let mut sa = [0u8; 16];
                sa[0..2].copy_from_slice(&2u16.to_le_bytes());
                sa[2..4].copy_from_slice(&8080u16.to_be_bytes());
                syscall3(SYS_BIND, fd, sa.as_ptr() as u64, 16);
                syscall3(SYS_LISTEN, fd, 4, 0);
                // NAT 规则：tcp 20001 -> 8080（网关控制面添加）
                let rc = syscall3(SYS_NAT_ADD, 6, 20001, 8080);
                print("natdemo: rule add rc=");
                print_u64(rc);
                print("\n");
                print("natdemo: listening on 8080\n");
                // accept：等宿主经 hostfwd(20001) 连接（DNAT 投递到 8080）
                let mut afd: u64 = 0;
                while afd == 0 {
                    afd = syscall3(SYS_ACCEPT, fd, 0, 0);
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("natdemo: accepted via DNAT fd=");
                print_u64(afd);
                print("\n");
                // recv：等宿主数据
                let mut rb = [0u8; 128];
                let mut got: u64 = 0;
                while got == 0 {
                    got = syscall6(SYS_RECVFROM, afd, rb.as_mut_ptr() as u64, 128, 0, 0, 0);
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("natdemo: recv ");
                print_u64(got);
                print("B: ");
                print(unsafe { core::str::from_utf8_unchecked(&rb[..got as usize]) });
                print("\n");
                // echo 回发（出向反向 NAT：8080 -> 20001）
                let n = syscall6(SYS_SENDTO, afd, rb.as_mut_ptr() as u64, got, 0, 0, 0);
                print("natdemo: echoed ");
                print_u64(n);
                print("\n");
                // conntrack 会话统计
                let mut st = [0u8; 16];
                syscall3(SYS_CT_STAT, st.as_mut_ptr() as u64, 0, 0);
                let entries = u64::from_le_bytes([st[0], st[1], st[2], st[3], st[4], st[5], st[6], st[7]]);
                let hits = u64::from_le_bytes([st[8], st[9], st[10], st[11], st[12], st[13], st[14], st[15]]);
                print("natdemo: ct entries=");
                print_u64(entries);
                print(" hits=");
                print_u64(hits);
                print("\n");
                syscall3(SYS_CLOSE, afd, 0, 0);
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
        }
        b"fwtest" => {
            // M8-切片3：基础防火墙——线性规则表 DROP/ACCEPT 验证（UDP hostfwd 回环 12343→19998）。
            let fd = syscall3(SYS_SOCKET, 2, 2, 0); // AF_INET, SOCK_DGRAM
            if (fd as i64) < 0 {
                print("fwtest: socket failed\n");
            } else {
                let mut sa = [0u8; 16];
                sa[0..2].copy_from_slice(&2u16.to_le_bytes());
                sa[2..4].copy_from_slice(&19998u16.to_be_bytes());
                syscall3(SYS_BIND, fd, sa.as_ptr() as u64, 16);
                // 1) DROP 规则：udp 19998
                let ar = syscall3(SYS_FW_ADD, 17, 19998, 0);
                print("fwtest: add drop rc=");
                print_u64(ar);
                print("\n");
                // 2) 发第 1 包（应被防火墙丢弃，回环不落地）
                let mut da = [0u8; 16];
                da[0..2].copy_from_slice(&2u16.to_le_bytes());
                da[2..4].copy_from_slice(&12343u16.to_be_bytes());
                da[4..8].copy_from_slice(&[10, 0, 2, 2]);
                let m1 = b"fw-drop-test";
                let sr = syscall6(SYS_SENDTO, fd, m1.as_ptr() as u64, m1.len() as u64, 0, da.as_ptr() as u64, 16);
                print("fwtest: sent1 rc=");
                print_u64(sr);
                print("\n");
                // 3) 轮询：等回环包到达并被防火墙丢弃（drops>=1），同时确认无泄漏
                let mut got_any = 0u64;
                let mut drops = 0u64;
                let mut rules = 0u64;
                let mut tries = 0u32;
                while tries < 400 && got_any == 0 && drops == 0 {
                    let mut rb = [0u8; 128];
                    let n = syscall6(SYS_RECVFROM, fd, rb.as_mut_ptr() as u64, 128, 0, 0, 0);
                    if (n as i64) > 0 {
                        got_any = n;
                        break;
                    }
                    let mut st = [0u8; 16];
                    syscall3(SYS_FW_STAT, st.as_mut_ptr() as u64, 0, 0);
                    rules = u64::from_le_bytes([st[0], st[1], st[2], st[3], st[4], st[5], st[6], st[7]]);
                    drops = u64::from_le_bytes([st[8], st[9], st[10], st[11], st[12], st[13], st[14], st[15]]);
                    tries += 1;
                    let mut spin = 0u32;
                    while spin < 2_000_000 {
                        spin += 1;
                    }
                }
                print("fwtest: fw rules=");
                print_u64(rules);
                print(" drops=");
                print_u64(drops);
                print(" leaked=");
                print_u64(got_any);
                print("\n");
                if got_any == 0 && drops >= 1 {
                    print("fwtest: DROP works\n");
                    // 4) 删除规则，第 2 包应通过
                    let dr = syscall3(SYS_FW_DEL, 17, 19998, 0);
                    print("fwtest: del rc=");
                    print_u64(dr);
                    print("\n");
                    let m2 = b"fw-ok-data";
                    syscall6(SYS_SENDTO, fd, m2.as_ptr() as u64, m2.len() as u64, 0, da.as_ptr() as u64, 16);
                    let mut got2 = 0u64;
                    let mut t2 = 0u32;
                    while got2 == 0 && t2 < 300 {
                        let mut rb2 = [0u8; 128];
                        let n = syscall6(SYS_RECVFROM, fd, rb2.as_mut_ptr() as u64, 128, 0, 0, 0);
                        if (n as i64) > 0 {
                            got2 = n;
                            print("fwtest: recv after del: ");
                            print(unsafe { core::str::from_utf8_unchecked(&rb2[..n as usize]) });
                            print("\n");
                        }
                        t2 += 1;
                        let mut spin = 0u32;
                        while spin < 2_000_000 {
                            spin += 1;
                        }
                    }
                    if got2 > 0 {
                        print("fwtest: ACCEPT works\n");
                    } else {
                        print("fwtest: accept FAILED\n");
                    }
                } else {
                    print("fwtest: DROP FAILED\n");
                }
                syscall3(SYS_CLOSE, fd, 0, 0);
            }
        }
        b"proctest" => {
            // M8-切片4：容器内 /proc（pid ns 视图）——挂载 proc 后按当前 ns 列任务。
            syscall3(SYS_MKDIR, b"/proc\0".as_ptr() as u64, 0o755, 0);
            let rc = syscall5(SYS_MOUNT, b"proc\0".as_ptr() as u64, b"/proc\0".as_ptr() as u64, b"proc\0".as_ptr() as u64, 0, 0);
            print("proctest: mount rc=");
            print_u64(rc);
            print("\n");
            if rc == 0 {
                // 宿主视图：根 ns 至少含 pid 1（shell）
                let mut saw1 = false;
                let mut total = 0u64;
                let fd = syscall3(SYS_OPEN, b"/proc\0".as_ptr() as u64, 0, 0);
                loop {
                    let mut buf = [0u8; 512];
                    let n = syscall3(SYS_GETDENTS64, fd, buf.as_mut_ptr() as u64, 512);
                    if n == 0 {
                        break;
                    }
                    let mut off = 0usize;
                    while off + 19 <= n as usize {
                        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                        let mut nl = 0usize;
                        while off + 19 + nl < buf.len() && buf[off + 19 + nl] != 0 {
                            nl += 1;
                        }
                        if nl > 0 {
                            total += 1;
                            if nl == 1 && buf[off + 19] == b'1' {
                                saw1 = true;
                            }
                        }
                        off += reclen;
                    }
                }
                syscall3(SYS_CLOSE, fd, 0, 0);
                print("proctest: parent /proc entries=");
                print_u64(total);
                print(" has_pid1=");
                print_u64(saw1 as u64);
                print("\n");
                if saw1 {
                    print("proctest: parent sees pid 1\n");
                }
                // 新 pid ns 子进程：/proc 只看到自己（pid=1）
                let r = syscall3(SYS_CLONE, 0x2000_0000, 0, 0);
                if r == 0 {
                    let fd2 = syscall3(SYS_OPEN, b"/proc\0".as_ptr() as u64, 0, 0);
                    if (fd2 as i64) >= 0 {
                        let mut pids = 0u64;
                        let mut only1 = true;
                        loop {
                            let mut buf = [0u8; 512];
                            let n = syscall3(SYS_GETDENTS64, fd2, buf.as_mut_ptr() as u64, 512);
                            if n == 0 {
                                break;
                            }
                            let mut off = 0usize;
                            while off + 19 <= n as usize {
                                let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                                let mut nl = 0usize;
                                while off + 19 + nl < buf.len() && buf[off + 19 + nl] != 0 {
                                    nl += 1;
                                }
                                if nl > 0 {
                                    // 仅统计纯数字项（pid 目录；health/cpuinfo 非数字忽略）
                                    let mut numeric = nl > 0;
                                    let mut j = 0usize;
                                    while j < nl {
                                        let c = buf[off + 19 + j];
                                        if c < b'0' || c > b'9' {
                                            numeric = false;
                                            break;
                                        }
                                        j += 1;
                                    }
                                    if numeric {
                                        pids += 1;
                                        if !(nl == 1 && buf[off + 19] == b'1') {
                                            only1 = false;
                                        }
                                    }
                                }
                                off += reclen;
                            }
                        }
                        syscall3(SYS_CLOSE, fd2, 0, 0);
                        print("proctest: child ns /proc pids=");
                        print_u64(pids);
                        print(" only_self=");
                        print_u64(only1 as u64);
                        print("\n");
                        if pids == 1 && only1 {
                            print("proctest: child ns sees only self\n");
                        }
                    }
                    syscall3(SYS_EXIT, 0, 0, 0);
                } else {
                    let mut w = 0u64;
                    let mut n = 0u32;
                    while w == 0 && n < 200000 {
                        w = syscall3(SYS_WAITPID, r, 0, 0);
                        n += 1;
                        let mut s = 0u32;
                        while s < 50000 {
                            s += 1;
                        }
                    }
                    print("proctest: child reaped=");
                    print_u64(w);
                    print("\n");
                }
            }
        }
        b"healthtest" => {
            // M9-切片1：/proc/health（JSON 健康指标）+ /proc/cpuinfo（多核报告）。
            // 1) 基线 health
            let fd = syscall3(SYS_OPEN, b"/proc/health\0".as_ptr() as u64, 0, 0);
            if (fd as i64) < 0 {
                print("healthtest: open health failed\n");
            } else {
                let mut buf = [0u8; 256];
                let n = syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, 256);
                syscall3(SYS_CLOSE, fd, 0, 0);
                print("healthtest: base ");
                print(unsafe { core::str::from_utf8_unchecked(&buf[..n as usize]) });
                let s = unsafe { core::str::from_utf8_unchecked(&buf[..n as usize]) };
                let ok_json = s.contains("mem_used") && s.contains("mem_free")
                    && s.contains("fds") && s.contains("cpu_load");
                if ok_json {
                    print("healthtest: health json ok\n");
                } else {
                    print("healthtest: health json FAILED\n");
                }
            }
            // 2) cpuinfo：online 只显示 1（SMP 前避免误判）
            let fd2 = syscall3(SYS_OPEN, b"/proc/cpuinfo\0".as_ptr() as u64, 0, 0);
            if (fd2 as i64) < 0 {
                print("healthtest: open cpuinfo failed\n");
            } else {
                let mut buf2 = [0u8; 256];
                let n2 = syscall3(SYS_READ, fd2, buf2.as_mut_ptr() as u64, 256);
                syscall3(SYS_CLOSE, fd2, 0, 0);
                let s2 = unsafe { core::str::from_utf8_unchecked(&buf2[..n2 as usize]) };
                if s2.contains("processor\t: 0") && s2.contains("online\t\t: 1") {
                    print("healthtest: cpuinfo online=1 ok\n");
                } else {
                    print("healthtest: cpuinfo FAILED\n");
                }
            }
            // 3) 容器计数：fork 容器子进程（NEWPID）存活期间 containers>=1
            let r = syscall3(SYS_CLONE, 0x2000_0000, 0, 0);
            if r == 0 {
                // 容器 init：短时自旋（给父读 health 的窗口），然后退出
                let mut i = 0u32;
                while i < 4000 {
                    i += 1;
                    let mut spin = 0u32;
                    while spin < 100_000 {
                        spin += 1;
                    }
                }
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                // 父：子先执行（自旋中），此时读 health → containers 应为 1
                let fd3 = syscall3(SYS_OPEN, b"/proc/health\0".as_ptr() as u64, 0, 0);
                let mut buf3 = [0u8; 256];
                let n3 = syscall3(SYS_READ, fd3, buf3.as_mut_ptr() as u64, 256);
                syscall3(SYS_CLOSE, fd3, 0, 0);
                print("healthtest: with container ");
                print(unsafe { core::str::from_utf8_unchecked(&buf3[..n3 as usize]) });
                let s3 = unsafe { core::str::from_utf8_unchecked(&buf3[..n3 as usize]) };
                if s3.contains("\"containers\":1") {
                    print("healthtest: container counted ok\n");
                } else {
                    print("healthtest: container count FAILED\n");
                }
                // 回收容器 init
                let mut w = 0u64;
                let mut n = 0u32;
                while w == 0 && n < 300000 {
                    w = syscall3(SYS_WAITPID, r, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                print("healthtest: reaped=");
                print_u64(w);
                print("\n");
            }
        }
        b"blktest" => {
            // M10-切片1：virtio-blk + BIO——写扇区 → 读回验证（真实块设备介质）。
            let mut wbuf = [0u8; 512];
            let msg = b"blk-hello-shanshui-guanxin";
            wbuf[..msg.len()].copy_from_slice(msg);
            // 写 sector 2（0/1 留给后续文件系统）
            let wr = syscall6(SYS_BLK_RW, 2, wbuf.as_mut_ptr() as u64, msg.len() as u64, 1, 0, 0);
            print("blktest: write rc=");
            print_u64(wr);
            print("\n");
            let mut rbuf = [0u8; 512];
            let rd = syscall6(SYS_BLK_RW, 2, rbuf.as_mut_ptr() as u64, msg.len() as u64, 0, 0, 0);
            print("blktest: read rc=");
            print_u64(rd);
            print(" data=");
            print(unsafe { core::str::from_utf8_unchecked(&rbuf[..msg.len()]) });
            print("\n");
            if wr == 0 && rd == 0 && &rbuf[..msg.len()] == msg {
                print("blktest: sector roundtrip ok\n");
            } else {
                print("blktest: FAILED\n");
            }
            // 未写扇区（sector 9）应为全零（证明读到真实介质而非残留）
            let mut zbuf = [0u8; 512];
            let rz = syscall6(SYS_BLK_RW, 9, zbuf.as_mut_ptr() as u64, 16, 0, 0, 0);
            let all_zero = rz == 0 && zbuf[..16].iter().all(|&b| b == 0);
            print("blktest: zero-sector check=");
            print_u64(all_zero as u64);
            print("\n");
            if all_zero {
                print("blktest: fresh sector zero ok\n");
            }
        }
        b"ext4test" => {
            // M10-切片2：ext4-lite 持久化文件系统 + Page Cache——创建/读写/删除，
            // sync 落盘 + drop 缓存模拟重启后仍能读回（持久化）。
            let rc0 = syscall6(SYS_BLKFS, 0, 0, 0, 0, 0, 0);
            print("ext4test: init rc=");
            print_u64(rc0);
            print("\n");
            // 1) create + write + read
            let rc1 = syscall6(SYS_BLKFS, 1, b"persist.txt\0".as_ptr() as u64, 0, 0, 0, 0);
            print("ext4test: create rc=");
            print_u64(rc1);
            print("\n");
            let msg = b"disk-forever";
            let rc2 = syscall6(SYS_BLKFS, 2, b"persist.txt\0".as_ptr() as u64, msg.as_ptr() as u64, msg.len() as u64, 0, 0);
            print("ext4test: write rc=");
            print_u64(rc2);
            print("\n");
            let mut rb = [0u8; 128];
            let rn = syscall6(SYS_BLKFS, 3, b"persist.txt\0".as_ptr() as u64, rb.as_mut_ptr() as u64, 128, 0, 0);
            print("ext4test: read rc=");
            print_u64(rn);
            print(" data=");
            print(unsafe { core::str::from_utf8_unchecked(&rb[..rn as usize]) });
            print("\n");
            // 2) list
            let mut lb = [0u8; 256];
            let ln = syscall6(SYS_BLKFS, 7, 0, lb.as_mut_ptr() as u64, 256, 0, 0);
            print("ext4test: list: ");
            print(unsafe { core::str::from_utf8_unchecked(&lb[..ln as usize]) });
            print("\n");
            // 3) sync 落盘 + drop 缓存（模拟重启）+ 再读
            let rc3 = syscall6(SYS_BLKFS, 5, 0, 0, 0, 0, 0);
            print("ext4test: sync rc=");
            print_u64(rc3);
            print("\n");
            syscall6(SYS_BLKFS, 6, 0, 0, 0, 0, 0);
            print("ext4test: cache dropped\n");
            let mut rb2 = [0u8; 128];
            let rn2 = syscall6(SYS_BLKFS, 3, b"persist.txt\0".as_ptr() as u64, rb2.as_mut_ptr() as u64, 128, 0, 0);
            print("ext4test: after reboot read: ");
            print(unsafe { core::str::from_utf8_unchecked(&rb2[..rn2 as usize]) });
            print("\n");
            if rn2 as usize == msg.len() && &rb2[..msg.len()] == msg {
                print("ext4test: persisted across reboot\n");
            } else {
                print("ext4test: persist FAILED\n");
            }
            // 4) 多文件 + 删除
            syscall6(SYS_BLKFS, 1, b"second.txt\0".as_ptr() as u64, 0, 0, 0, 0);
            let m2 = b"file2";
            syscall6(SYS_BLKFS, 2, b"second.txt\0".as_ptr() as u64, m2.as_ptr() as u64, m2.len() as u64, 0, 0);
            let rc4 = syscall6(SYS_BLKFS, 4, b"second.txt\0".as_ptr() as u64, 0, 0, 0, 0);
            print("ext4test: unlink rc=");
            print_u64(rc4);
            print("\n");
            let mut lb2 = [0u8; 256];
            let ln2 = syscall6(SYS_BLKFS, 7, 0, lb2.as_mut_ptr() as u64, 256, 0, 0);
            print("ext4test: list after unlink: ");
            print(unsafe { core::str::from_utf8_unchecked(&lb2[..ln2 as usize]) });
            print("\n");
        }
        b"futtest" => {
            // M11-切片1：futex——父子进程共享内存同步（WAIT/WAKE）。
            // fork 共享用户地址空间，栈上 flag 对父子可见。
            let mut flag = 0u32;
            let addr = (&mut flag as *mut u32) as u64;
            let r = syscall3(SYS_FORK, 0, 0, 0);
            if r == 0 {
                // 子：等 flag 变为 1（futex WAIT 阻塞，父改值后 WAKE 唤醒）
                print("futtest: child waiting\n");
                let rc = syscall5(SYS_FUTEX, addr, 0, 0, 0, 0); // WAIT(addr, 0)
                print("futtest: child futex rc=");
                print_u64(rc);
                print(" flag=");
                print_u64(flag as u64);
                print("\n");
                if rc == 0 && flag == 1 {
                    print("futtest: child woke ok\n");
                }
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                // 父：子先执行已阻塞在 WAIT，改共享值 + WAKE 唤醒
                let mut spin = 0u32;
                while spin < 5_000_000 {
                    spin += 1;
                }
                flag = 1;
                let wr = syscall5(SYS_FUTEX, addr, 1, 1, 0, 0); // WAKE(addr, 1)
                print("futtest: parent wake rc=");
                print_u64(wr);
                print("\n");
                let mut w = 0u64;
                let mut n = 0u32;
                while w == 0 && n < 200000 {
                    w = syscall3(SYS_WAITPID, r, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                print("futtest: reaped=");
                print_u64(w);
                print("\n");
            }
        }
        b"tlstest" => {
            // M11-切片2：arch_prctl TLS——FS base 设置/查询 + 上下文切换恢复。
            // 1) ARCH_SET_FS：FS base 指向 TLS_BUF_A，经 %fs 写入/读回标记
            let base_a = unsafe { core::ptr::addr_of_mut!(TLS_BUF_A) as u64 };
            let rc = syscall3(SYS_ARCH_PRCTL, 0x1002, base_a, 0);
            print("tlstest: set fs rc=");
            print_u64(rc);
            print("\n");
            let marker: u64 = 0x1122_3344_5566_7788;
            // SAFETY: FS base 已设为 TLS_BUF_A（恒等映射），%fs:0 落于其内。
            unsafe {
                core::arch::asm!("mov qword ptr fs:[0], {0}", in(reg) marker, options(nostack));
            }
            let mut back: u64 = 0;
            // SAFETY: 同上；读回同一标记。
            unsafe {
                core::arch::asm!("mov {0}, qword ptr fs:[0]", out(reg) back, options(nostack));
            }
            print("tlstest: fs read back=");
            print_u64(back);
            print("\n");
            if back == marker {
                print("tlstest: fs rw ok\n");
            }
            // 2) ARCH_GET_FS：回读当前 FS base，应为 TLS_BUF_A
            let mut got: u64 = 0;
            let rc2 = syscall3(SYS_ARCH_PRCTL, 0x1003, &mut got as *mut u64 as u64, 0);
            print("tlstest: get fs rc=");
            print_u64(rc2);
            print(" base=");
            print_u64(got);
            print("\n");
            if rc2 == 0 && got == base_a {
                print("tlstest: get fs ok\n");
            }
            // 3) fork：子改 FS base 指向 TLS_BUF_B 并写不同标记；父回收后
            //    经 %fs 读自己的 TLS——必须仍是原标记（调度切换恢复 FS base）。
            let r = syscall3(SYS_FORK, 0, 0, 0);
            if r == 0 {
                let base_b = unsafe { core::ptr::addr_of_mut!(TLS_BUF_B) as u64 };
                let rc3 = syscall3(SYS_ARCH_PRCTL, 0x1002, base_b, 0);
                let cmarker: u64 = 0xDEAD_BEEF_0000_0001;
                // SAFETY: 子 FS base 已设为 TLS_BUF_B。
                unsafe {
                    core::arch::asm!("mov qword ptr fs:[0], {0}", in(reg) cmarker, options(nostack));
                }
                let mut cback: u64 = 0;
                // SAFETY: 同上。
                unsafe {
                    core::arch::asm!("mov {0}, qword ptr fs:[0]", out(reg) cback, options(nostack));
                }
                print("tlstest: child fs back=");
                print_u64(cback);
                print("\n");
                if rc3 == 0 && cback == cmarker {
                    print("tlstest: child own fs ok\n");
                }
                // 子退出；父随调度器恢复自己的 FS base（restore_fs_base 路径）
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                let mut w = 0u64;
                let mut n = 0u32;
                while w == 0 && n < 200000 {
                    w = syscall3(SYS_WAITPID, r, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                let mut pback: u64 = 0;
                // SAFETY: 父 FS base 应已被调度器恢复为 TLS_BUF_A。
                unsafe {
                    core::arch::asm!("mov {0}, qword ptr fs:[0]", out(reg) pback, options(nostack));
                }
                print("tlstest: parent fs after child=");
                print_u64(pback);
                print("\n");
                if pback == marker {
                    print("tlstest: ctx switch fs restore ok\n");
                }
                print("tlstest: reaped=");
                print_u64(w);
                print("\n");
            }
        }
        b"clonetest" => {
            // M11-切片3：clone CLONE_SETTLS + CLONE_CHILD_CLEARTID——pthread 原语。
            // CLONE_SETTLS：子 FS base 直接取 clone 的 tls 参数；
            // CLONE_CHILD_CLEARTID：子退出时内核清零 tid。
            // 调度约束：shell(task 0) 仅在就绪树为空时运行，故子须先阻塞
            // （futex WAIT(go)）让出 CPU，父唤醒它后再登记自己的等待。
            let stack_top = unsafe { (core::ptr::addr_of_mut!(CLONE_STACK) as usize + 8192) as u64 };
            let tid_addr = unsafe { core::ptr::addr_of_mut!(CLONE_TID) as u64 };
            let tls_addr = unsafe { core::ptr::addr_of_mut!(CLONE_TLS) as u64 };
            let go_addr = unsafe { core::ptr::addr_of_mut!(CLONE_GO) as u64 };
            // SAFETY: 单核测试环境，写初值。
            unsafe {
                core::ptr::write_volatile(core::ptr::addr_of_mut!(CLONE_TID) as *mut u32, 0xDEAD_0001);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(CLONE_GO) as *mut u32, 0);
            }
            let flags: u64 = 0x0008_0000 | 0x0020_0000; // CLONE_SETTLS | CLONE_CHILD_CLEARTID
            // clone(flags, stack, parent_tidptr=0, child_tidptr=tid, tls)
            let r = syscall6(SYS_CLONE, flags, stack_top, 0, tid_addr, tls_addr, 0);
            if r == 0 {
                // 子：先 futex WAIT(go) 阻塞（让出 CPU），父唤醒后继续
                syscall5(SYS_FUTEX, go_addr, 0, 0, 0, 0); // WAIT(go, 0)
                // FS base 应已被 CLONE_SETTLS 设为 tls_addr（不经 ARCH_SET_FS）
                let mut got: u64 = 0;
                syscall3(SYS_ARCH_PRCTL, 0x1003, &mut got as *mut u64 as u64, 0);
                print("clonetest: child get fs=");
                print_u64(got);
                print("\n");
                if got == tls_addr {
                    print("clonetest: child settls ok\n");
                }
                // 经 %fs 写子自己的 TLS 并读回（调度切换时已恢复子 FS base 到 MSR）
                let m: u64 = 0xCAFE_0000_0000_0001;
                // SAFETY: FS base 已由调度器恢复为 tls_addr（CLONE_SETTLS）。
                unsafe {
                    core::arch::asm!("mov qword ptr fs:[0], {0}", in(reg) m, options(nostack));
                }
                let mut b: u64 = 0;
                // SAFETY: 同上。
                unsafe {
                    core::arch::asm!("mov {0}, qword ptr fs:[0]", out(reg) b, options(nostack));
                }
                print("clonetest: child fs back=");
                print_u64(b);
                print("\n");
                if b == m {
                    print("clonetest: child tls rw ok\n");
                }
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                // 父：子已阻塞在 WAIT(go) → 置 go 并 WAKE，让子做 TLS 验证
                // SAFETY: 单核测试环境。
                unsafe {
                    core::ptr::write_volatile(core::ptr::addr_of_mut!(CLONE_GO) as *mut u32, 1);
                }
                syscall5(SYS_FUTEX, go_addr, 1, 1, 0, 0); // WAKE(go, 1)
                // 等子退出（waitpid 轮询）；退出时内核清零 tid（CLONE_CHILD_CLEARTID）
                let mut w = 0u64;
                let mut n = 0u32;
                while w == 0 && n < 200000 {
                    w = syscall3(SYS_WAITPID, r, 0, 0);
                    n += 1;
                    let mut s = 0u32;
                    while s < 50000 {
                        s += 1;
                    }
                }
                // SAFETY: 读清零后的 tid。
                let cleared =
                    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CLONE_TID) as *const u32) };
                print("clonetest: tid after child exit=");
                print_u64(cleared as u64);
                print("\n");
                if cleared == 0 {
                    print("clonetest: child cleartid ok\n");
                }
                print("clonetest: reaped=");
                print_u64(w);
                print("\n");
            }
        }
        b"reqtest" => {
            // M11-切片4：futex REQUEUE + 超时（ETIMEDOUT）。
            // 调度约束：shell(task 0) 仅在就绪树为空时运行，故子先 WAIT(go)
            // 阻塞让出 CPU，父逐个放行并推进。
            // SAFETY: 单核测试环境，复位共享状态。
            unsafe {
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_CV) as *mut u32, 0);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_MUTEX) as *mut u32, 0);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_GO_A) as *mut u32, 0);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_GO_B) as *mut u32, 0);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_RDY_B) as *mut u32, 0);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_TM) as *mut u32, 0);
                core::ptr::write_volatile(core::ptr::addr_of_mut!(RQ_WOKE) as *mut u32, 0);
            }
            let cv = unsafe { core::ptr::addr_of_mut!(RQ_CV) as u64 };
            let mutex = unsafe { core::ptr::addr_of_mut!(RQ_MUTEX) as u64 };
            let goa = unsafe { core::ptr::addr_of_mut!(RQ_GO_A) as u64 };
            let gob = unsafe { core::ptr::addr_of_mut!(RQ_GO_B) as u64 };
            let rdyb = unsafe { core::ptr::addr_of_mut!(RQ_RDY_B) as u64 };
            let tm = unsafe { core::ptr::addr_of_mut!(RQ_TM) as u64 };
            let woke = unsafe { core::ptr::addr_of_mut!(RQ_WOKE) as u64 };
            // ---- REQUEUE 部分：A、B 双等待者阻塞在 cv ----
            let ra = syscall3(SYS_FORK, 0, 0, 0);
            if ra == 0 {
                // 子 A：先阻塞等父放行，再 WAIT(cv)
                syscall5(SYS_FUTEX, goa, 0, 0, 0, 0);
                syscall5(SYS_FUTEX, cv, 0, 0, 0, 0); // WAIT(cv, 0)
                // SAFETY: 单核，volatile 自增唤醒计数。
                unsafe {
                    core::ptr::write_volatile(
                        woke as *mut u32,
                        core::ptr::read_volatile(woke as *const u32) + 1,
                    );
                }
                print("reqtest: child A woke\n");
                syscall3(SYS_EXIT, 0, 0, 0);
            } else {
                syscall5(SYS_FUTEX, goa, 1, 1, 0, 0); // WAKE(goA) 放行 A
                let rb = syscall3(SYS_FORK, 0, 0, 0);
                if rb == 0 {
                    // 子 B：阻塞到 cv 前置位 rdyB（父据此确认 B 已就位）
                    syscall5(SYS_FUTEX, gob, 0, 0, 0, 0);
                    // SAFETY: 单核。
                    unsafe { core::ptr::write_volatile(rdyb as *mut u32, 1) };
                    syscall5(SYS_FUTEX, cv, 0, 0, 0, 0);
                    // SAFETY: 单核，volatile 自增唤醒计数。
                    unsafe {
                        core::ptr::write_volatile(
                            woke as *mut u32,
                            core::ptr::read_volatile(woke as *const u32) + 1,
                        );
                    }
                    print("reqtest: child B woke\n");
                    syscall3(SYS_EXIT, 0, 0, 0);
                } else {
                    syscall5(SYS_FUTEX, gob, 1, 1, 0, 0); // WAKE(goB) 放行 B
                    // 等 B 阻塞到 cv（父仅在树空时运行 → 此刻 B 必已阻塞）；
                    // black_box 防 release 优化删掉纯循环，确保跨越多拍等 B 就位
                    let mut n1 = 0u32;
                    while unsafe { core::ptr::read_volatile(rdyb as *const u32) } == 0
                        && n1 < 30_000_000
                    {
                        n1 = n1.wrapping_add(1);
                        core::hint::black_box(n1);
                    }
                    // CMP_REQUEUE(cv, wake=1, mutex, cmp=0)：唤醒 A，把 B 搬上 mutex
                    let rq = syscall5(SYS_FUTEX, cv, 4, 1, mutex, 0);
                    print("reqtest: cmp_requeue rc=");
                    print_u64(rq);
                    print("\n");
                    // WAKE(mutex)：唤醒被 requeue 的 B
                    let mw = syscall5(SYS_FUTEX, mutex, 1, 1, 0, 0);
                    print("reqtest: mutex wake rc=");
                    print_u64(mw);
                    print("\n");
                    // 回收 A、B
                    let mut w = 0u64;
                    let mut n = 0u32;
                    while w == 0 && n < 200000 {
                        w = syscall3(SYS_WAITPID, ra, 0, 0);
                        n += 1;
                        let mut s = 0u32;
                        while s < 5000 {
                            s += 1;
                        }
                    }
                    let mut wb = 0u64;
                    let mut n2 = 0u32;
                    while wb == 0 && n2 < 200000 {
                        wb = syscall3(SYS_WAITPID, rb, 0, 0);
                        n2 += 1;
                        let mut s = 0u32;
                        while s < 5000 {
                            s += 1;
                        }
                    }
                    // SAFETY: 单核读。
                    let cnt = unsafe { core::ptr::read_volatile(woke as *const u32) };
                    print("reqtest: total woke=");
                    print_u64(cnt as u64);
                    print("\n");
                    // ---- 超时部分：C 阻塞 2 tick 后返回 ETIMEDOUT ----
                    let rc = syscall3(SYS_FORK, 0, 0, 0);
                    if rc == 0 {
                        let tr = syscall5(SYS_FUTEX, tm, 0, 0, 2, 0); // WAIT(tm,0,timeout=2)
                        print("reqtest: timeout rc=");
                        print_u64(tr);
                        print("\n");
                        if tr == 110 {
                            print("reqtest: etimedout ok\n");
                        }
                        syscall3(SYS_EXIT, 0, 0, 0);
                    } else {
                        let mut wc = 0u64;
                        let mut n3 = 0u32;
                        while wc == 0 && n3 < 200000 {
                            wc = syscall3(SYS_WAITPID, rc, 0, 0);
                            n3 += 1;
                            let mut s = 0u32;
                            while s < 5000 {
                                s += 1;
                            }
                        }
                        print("reqtest: reaped C=");
                        print_u64(wc);
                        print("\n");
                    }
                }
            }
        }
        b"maptest" => {
            // M13-01：/proc/self/maps——当前进程地址空间映射（ELF 段 + 用户栈）。
            // 注：proctest 已挂载 /proc；这里直接读取。
            let fd = syscall3(SYS_OPEN, b"/proc/self/maps\0".as_ptr() as u64, 0, 0);
            if fd >= 10000 {
                // SYS_OPEN 失败：错误码为负值按 u64 是大数
                print("maptest: open failed rc=");
                print_u64(fd);
                print("\n");
            } else {
                let mut total = 0usize;
                // SAFETY: 单核测试环境。
                let buf: &mut [u8; 4096] = unsafe { &mut *core::ptr::addr_of_mut!(MAP_BUF) };
                loop {
                    let n = syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, 4096);
                    if n <= 0 {
                        break;
                    }
                    total += n as usize;
                }
                syscall3(SYS_CLOSE, fd, 0, 0);
                // 统计行数与关键内容
                let mut lines = 0u64;
                let mut i = 0usize;
                while i < total {
                    if buf[i] == b'\n' {
                        lines += 1;
                    }
                    i += 1;
                }
                let has_init = scan_bytes(&buf[..total], b"/init");
                let has_stack = scan_bytes(&buf[..total], b"[stack]");
                let has_rxp = scan_bytes(&buf[..total], b"r-xp");
                print("maptest: bytes=");
                print_u64(total as u64);
                print(" lines=");
                print_u64(lines);
                print(" init=");
                print_u64(has_init as u64);
                print(" stack=");
                print_u64(has_stack as u64);
                print("\n");
                // 打印首行（映射范围 + 权限）
                let mut j = 0usize;
                while j < total && buf[j] != b'\n' {
                    j += 1;
                }
                print("maptest: first=");
                // SAFETY: 首行全为 ASCII。
                print(unsafe { core::str::from_utf8_unchecked(&buf[..j]) });
                print("\n");
                if lines >= 6 && has_init && has_stack && has_rxp {
                    print("maptest: maps ok\n");
                } else {
                    print("maptest: maps FAIL\n");
                }
            }
        }
        b"statustest" => {
            // M13-02：/proc/self/status——进程状态（VmRSS/VmPeak/Threads/Uid/Gid）。
            let fd = syscall3(SYS_OPEN, b"/proc/self/status\0".as_ptr() as u64, 0, 0);
            if fd >= 10000 {
                // SYS_OPEN 失败：错误码为负值按 u64 是大数
                print("statustest: open failed rc=");
                print_u64(fd);
                print("\n");
            } else {
                let mut total = 0usize;
                // SAFETY: 单核测试环境。
                let buf: &mut [u8; 4096] = unsafe { &mut *core::ptr::addr_of_mut!(MAP_BUF) };
                loop {
                    // 内核 SYS_READ 每次最多 512B，须按偏移累积，避免覆盖已读内容
                    let n = syscall3(
                        SYS_READ,
                        fd,
                        (buf.as_mut_ptr() as u64) + total as u64,
                        (4096 - total) as u64,
                    );
                    if n <= 0 {
                        break;
                    }
                    total += n as usize;
                }
                syscall3(SYS_CLOSE, fd, 0, 0);
                let has_name = scan_bytes(&buf[..total], b"Name:");
                let has_uid = scan_bytes(&buf[..total], b"Uid:");
                let has_vmrss = scan_bytes(&buf[..total], b"VmRSS:");
                let has_threads = scan_bytes(&buf[..total], b"Threads:");
                print("statustest: bytes=");
                print_u64(total as u64);
                print(" name=");
                print_u64(has_name as u64);
                print(" uid=");
                print_u64(has_uid as u64);
                print(" vmrss=");
                print_u64(has_vmrss as u64);
                print(" threads=");
                print_u64(has_threads as u64);
                print("\n");
                // 打印 VmRSS 行（关键指标）
                let s = core::str::from_utf8(&buf[..total]).unwrap_or("");
                if let Some(pos) = s.find("VmRSS:") {
                    let end = s[pos..].find('\n').map(|e| pos + e).unwrap_or(total);
                    print("statustest: ");
                    print(&s[pos..end]);
                    print("\n");
                }
                if has_name && has_uid && has_vmrss && has_threads {
                    print("statustest: status ok\n");
                } else {
                    print("statustest: status FAIL\n");
                }
            }
        }
        b"sigtest" => {
            // M13-06：注册 SIGSEGV handler → 触发用户态 #PF → handler 改恢复点 →
            // rt_sigreturn → 回到 sigtest 继续验证（内核不 panic）。
            let act = SigAction {
                handler: segv_handler as usize as u64,
                flags: SA_SIGINFO,
                restorer: 0,
                mask: 0,
            };
            let rc = syscall6(SYS_RT_SIGACTION, SIGSEGV, &act as *const SigAction as u64, 0, 8, 0, 0);
            print("sigtest: sigaction rc=");
            print_u64(rc);
            print("\n");
            // 保存恢复点（handler 经 rt_sigreturn 后从 jmp_set 调用点继续）
            jmp_set();
            if !unsafe { SIG_ACTIVE } {
                // 首次：触发 #PF
                // SAFETY: 单核测试环境。
                unsafe { SIG_ACTIVE = true; }
                print("sigtest: triggering page fault (write addr 0)...\n");
                // SAFETY: 故意写非法地址 0，触发用户态 #PF → SIGSEGV 投递。
                unsafe { core::ptr::write_volatile(0usize as *mut u64, 1) };
            }
            // 恢复后走到这里：验证 handler 结果
            print("sigtest: resumed after SIGSEGV\n");
            // SAFETY: 单核测试环境。
            let signos = unsafe { HANDLED_SIGNOS };
            let addr = unsafe { HANDLED_ADDR };
            print("sigtest: handler ran, signos=");
            print_u64(signos);
            print(" addr=");
            print_u64(addr);
            print("\n");
            if signos == SIGSEGV && addr == 0 {
                print("sigtest: SIGSEGV handled ok\n");
            } else {
                print("sigtest: SIGSEGV MISMATCH\n");
            }
        }
        b"exit" => {
            print("bye\n");
            syscall3(SYS_EXIT, 0, 0, 0);
        }
        b"pwd" => {
            let mut cwd = [0u8; 64];
            let n = syscall3(SYS_GETCWD, cwd.as_mut_ptr() as u64, 64, 0);
            if (n as i64) < 0 {
                print("pwd: failed\n");
            } else {
                print(unsafe { core::str::from_utf8_unchecked(&cwd[..n as usize]) });
                print("\n");
            }
        }
        _ if cmd.starts_with(b"cd ") => {
            let p = path_arg(cmd, 3);
            let rc = syscall3(SYS_CHDIR, p.as_ptr() as u64, 0, 0);
            if (rc as i64) < 0 {
                print("cd: rc=");
                print_u64(rc);
                print("\n");
            }
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
    print("\nShanshui-guanxin M3 userspace shell (init)\n");
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
