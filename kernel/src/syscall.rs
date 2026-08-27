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
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_WAITPID: u64 = 61;
pub const SYS_UNAME: u64 = 63;
pub const SYS_SETHOSTNAME: u64 = 170;
/// 山水观心操作系统扩展：读当前 cgroup 统计 { pids, mem }。
pub const SYS_CGROUP_STAT: u64 = 500;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;
pub const SYS_MOUNT: u64 = 165;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_FUTEX: u64 = 202;
/// arch_prctl：TLS 段基址（FS base）设置/查询（M11-切片2）。
pub const SYS_ARCH_PRCTL: u64 = 158;
/// 山水观心操作系统扩展：添加 NAT 端口映射规则（网关控制面）。
pub const SYS_NAT_ADD: u64 = 501;
/// 山水观心操作系统扩展：读 conntrack 统计 { 条目数, 命中数 }。
pub const SYS_CT_STAT: u64 = 502;
/// 山水观心操作系统扩展：添加防火墙规则（proto, dport, action）。
pub const SYS_FW_ADD: u64 = 503;
/// 山水观心操作系统扩展：删除防火墙规则（proto, dport）。
pub const SYS_FW_DEL: u64 = 504;
/// 山水观心操作系统扩展：读防火墙统计 { 规则数, 丢弃包数 }。
pub const SYS_FW_STAT: u64 = 505;
/// 山水观心操作系统扩展：BIO 扇区读写（lba, buf, len, is_write）。
pub const SYS_BLK_RW: u64 = 506;
/// 山水观心操作系统扩展：ext4-lite 文件系统操作（op, name, buf, len, offset）。
pub const SYS_BLKFS: u64 = 507;

/// boot.asm 导出的 syscall 入口。
#[cfg(not(test))]
extern "C" {
    fn syscall_entry();
}
// host 单测桩：boot.asm 不参与测试链接（init 不会被测试调用，仅需符号可解析）。
#[cfg(test)]
fn syscall_entry() {}

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
    // M6-切片1：fork/clone 需要当前用户上下文（帧），单独处理。
    // 子任务帧 = 本帧拷贝（rax 置 0）；子先执行（直接切到子帧），
    // 子退出后调度器经父帧恢复父进程（父返回子任务 id）。
    if nr == SYS_FORK {
        // SAFETY: f 为当前 syscall 帧（用户上下文），user_fork 仅拷贝不修改。
        let cid = unsafe { crate::task::user_fork(f, 0, 0, 0) };
        f.rax = cid as u64; // 父侧返回值
        if cid > 0 {
            // 父帧登记为 ctx_rsp（父 syscall 未完成，保留在其内核栈上）；
            // 切 CURRENT 到子任务，并切 tss_rsp0 到子栈（子 syscall 不覆盖父帧）。
            crate::task::save_ctx(f as *const _ as usize);
            crate::task::set_current(cid as usize);
            crate::gdt::set_rsp0(crate::task::task_kstack_top(cid as usize));
            return crate::task::task_ctx(cid as usize) as *mut ExceptionFrame;
        }
    } else if nr == SYS_CLONE {
        // SAFETY: f 为当前 syscall 帧；Linux clone：rdi=flags, rsi=stack,
        // rdx=parent_tidptr, r10=child_tidptr, r8=tls（M11-切片3）。
        let cid = unsafe { crate::task::user_fork(f, f.rdi as u32, f.r10, f.r8) };
        f.rax = cid as u64;
        if cid > 0 {
            crate::task::save_ctx(f as *const _ as usize);
            crate::task::set_current(cid as usize);
            crate::gdt::set_rsp0(crate::task::task_kstack_top(cid as usize));
            return crate::task::task_ctx(cid as usize) as *mut ExceptionFrame;
        }
    } else {
        let ret = dispatch(nr, f.rdi, f.rsi, f.rdx, f.r10, f.r8, f.r9);
        f.rax = ret; // 返回值写回 rax
    }
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
        SYS_SOCKET => sys_socket(a1, a2, a3),
        SYS_CONNECT => sys_connect(a1, a2, a3),
        SYS_ACCEPT => sys_accept(a1, a2, a3),
        SYS_LISTEN => sys_listen(a1, a2),
        SYS_BIND => sys_bind(a1, a2, a3),
        SYS_SENDTO => sys_sendto(a1, a2, a3, a4, a5, a6),
        SYS_RECVFROM => sys_recvfrom(a1, a2, a3, a4, a5, a6),
        SYS_EPOLL_CREATE => sys_epoll_create(a1),
        SYS_EPOLL_CTL => sys_epoll_ctl(a1, a2, a3, a4),
        SYS_EPOLL_WAIT => sys_epoll_wait(a1, a2, a3, a4),
        SYS_MKDIR => sys_mkdir(a1, a2),
        SYS_RMDIR => sys_rmdir(a1),
        SYS_UNLINK => sys_unlink(a1),
        SYS_GETCWD => sys_getcwd(a1, a2),
        SYS_CHDIR => sys_chdir(a1),
        SYS_FUTEX => sys_futex(a1, a2, a3, a4, a5),
        SYS_ARCH_PRCTL => sys_arch_prctl(a1, a2),
        SYS_NAT_ADD => sys_nat_add(a1, a2, a3),
        SYS_CT_STAT => sys_ct_stat(a1),
        SYS_FW_ADD => sys_fw_add(a1, a2, a3),
        SYS_FW_DEL => sys_fw_del(a1, a2),
        SYS_FW_STAT => sys_fw_stat(a1),
        SYS_BLK_RW => sys_blk_rw(a1, a2, a3, a4),
        SYS_BLKFS => sys_blkfs(a1, a2, a3, a4, a5),
        SYS_MOUNT => sys_mount(a1, a2, a3, a4, a5),
        SYS_GETDENTS64 => sys_getdents64(a1, a2, a3),
        SYS_GETPID => sys_getpid(),
        SYS_WAITPID => sys_waitpid(a1, a2, a3),
        SYS_UNAME => sys_uname(a1),
        SYS_SETHOSTNAME => sys_sethostname(a1, a2),
        SYS_CGROUP_STAT => sys_cgroup_stat(a1),
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
    let name = match copy_path(path) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match crate::fs::open_path(&name, flags) {
        Ok(f) => {
            crate::fs::fd_alloc(f) as u64
        }
        Err(e) => e as u64, // 负 errno（按 Linux ABI 返回 -errno）
    }
}

/// close(fd)：关闭并回收 fd；成功返回 0。
fn sys_close(fd: u64) -> u64 {
    if fd >= crate::socket::EPOLL_FD_BASE as u64 {
        crate::socket::epoll_close(fd as usize) as u64
    } else if fd >= crate::socket::TCP_FD_BASE as u64 {
        crate::socket::tcp_close(fd as usize) as u64
    } else if fd >= 100 {
        crate::socket::udp_close(fd as usize) as u64
    } else if crate::fs::fd_close(fd as usize) {
        0
    } else {
        (-1i64) as u64 // EBADF
    }
}

/// 拷贝用户态路径到 String 并解析为绝对路径（M8-切片1：相对路径以 cwd 为前缀，
/// 处理 "." / ".."）。
fn copy_path(path: u64) -> Result<alloc::string::String, u64> {
    let mut pbuf = [0u8; 256];
    // SAFETY: path 为用户态 NUL 结尾字符串。
    let n = unsafe { copy_cstr_from_user(path, &mut pbuf) };
    // SAFETY: pbuf[..n] 为合法 UTF-8（ASCII 路径）。
    let raw = unsafe { core::str::from_utf8_unchecked(&pbuf[..n]) };
    Ok(resolve_abs(raw))
}

/// 相对路径 → 绝对路径：以当前任务 cwd 为前缀，逐组件处理 "." / ".."。
fn resolve_abs(path: &str) -> alloc::string::String {
    if path.starts_with('/') {
        return alloc::string::String::from(path);
    }
    let cwd = crate::task::current_cwd();
    // SAFETY: cwd 为内核内部 NUL 结尾 ASCII。
    let cwd_str = unsafe { core::str::from_utf8_unchecked(cwd) };
    let mut parts: alloc::vec::Vec<&str> =
        cwd_str.split('/').filter(|c| !c.is_empty()).collect();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        alloc::string::String::from("/")
    } else {
        let mut s = alloc::string::String::from("/");
        s.push_str(&parts.join("/"));
        s
    }
}

/// mkdir(path, mode)：创建目录；成功 0，失败负 errno。
fn sys_mkdir(path: u64, _mode: u64) -> u64 {
    let name = match copy_path(path) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match crate::fs::create_dir(&name) {
        Ok(()) => 0,
        Err(e) => e as u64,
    }
}

/// getcwd(buf, size)：拷贝当前任务 cwd（NUL 结尾）到用户缓冲；返回长度（M8-切片1）。
fn sys_getcwd(buf: u64, size: u64) -> u64 {
    let cwd = crate::task::current_cwd();
    let need = cwd.len() + 1;
    if (size as usize) < need {
        return (-34i64) as u64; // ERANGE
    }
    // SAFETY: buf 为用户态可写缓冲区，size 已校验 ≥ need。
    unsafe {
        core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf as *mut u8, cwd.len());
        *(buf as *mut u8).add(cwd.len()) = 0;
    }
    cwd.len() as u64
}

/// chdir(path)：切换当前任务工作目录（M8-切片1）；成功 0。
fn sys_chdir(path: u64) -> u64 {
    let name = match copy_path(path) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match crate::fs::stat_path(&name) {
        Ok((_, mode, _, _)) if mode & crate::fs::S_IFDIR != 0 => {
            match crate::task::set_cwd(&name) {
                Ok(()) => 0,
                Err(e) => e as u64,
            }
        }
        Ok(_) => (-20i64) as u64, // ENOTDIR
        Err(e) => e as u64,
    }
}

/// futex(addr, op, val, arg4, arg5)：共享内存同步（M11-切片1/4）。
/// WAIT 时 arg4 为超时 tick（0 = 无限）；REQUEUE/CMP_REQUEUE 用 arg4/arg5。
fn sys_futex(addr: u64, op: u64, val: u64, arg4: u64, arg5: u64) -> u64 {
    // SAFETY: 用户地址恒等映射，由 futex 模块校验语义。
    unsafe { crate::futex::futex(addr, op, val, arg4, arg5) }
}

/// arch_prctl(code, addr)：TLS 段基址（FS base）设置/查询（M11-切片2）。
///
/// - ARCH_SET_FS(0x1002)：addr 即新的 FS base，写入任务 TLS 字段并写 MSR。
/// - ARCH_GET_FS(0x1003)：把当前 FS base 写入 *(u64*)addr（用户恒等映射）。
fn sys_arch_prctl(code: u64, addr: u64) -> u64 {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    match code {
        ARCH_SET_FS => {
            crate::task::set_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            let v = crate::task::get_fs_base();
            // SAFETY: 用户地址恒等映射（与 sys_write 相同约束）。
            unsafe { core::ptr::write_volatile(addr as *mut u64, v) };
            0
        }
        _ => (-22i64) as u64, // EINVAL
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

/// socket(domain, type, protocol)：AF_INET + SOCK_STREAM(1)/SOCK_DGRAM(2)。
fn sys_socket(domain: u64, typ: u64, _proto: u64) -> u64 {
    if domain != 2 {
        return (-1i64) as u64; // EAFNOSUPPORT
    }
    match typ {
        1 => crate::socket::tcp_socket() as u64, // SOCK_STREAM（M5-切片4）
        2 => crate::socket::udp_socket() as u64, // SOCK_DGRAM
        _ => (-1i64) as u64,                     // EPROTONOSUPPORT
    }
}

/// bind(fd, sockaddr_in, len)：读 sockaddr_in 取端口（family@0 port@2 BE）。
fn sys_bind(fd: u64, addr: u64, _len: u64) -> u64 {
    let mut sa = [0u8; 16];
    // SAFETY: addr 为用户态 sockaddr_in（16 字节可读）。
    unsafe { core::ptr::copy_nonoverlapping(addr as *const u8, sa.as_mut_ptr(), 16) };
    let port = u16::from_be_bytes([sa[2], sa[3]]);
    if fd >= crate::socket::TCP_FD_BASE as u64 {
        crate::socket::tcp_bind(fd as usize, port) as u64
    } else {
        crate::socket::udp_bind(fd as usize, port) as u64
    }
}

/// listen(fd, backlog)：TCP 监听。
fn sys_listen(fd: u64, backlog: u64) -> u64 {
    crate::socket::tcp_listen(fd as usize, backlog as usize) as u64
}

/// accept(fd, addr, addrlen)：非阻塞取已建立连接 fd；无则 0。
fn sys_accept(fd: u64, _addr: u64, _addrlen: u64) -> u64 {
    crate::socket::tcp_accept(fd as usize) as u64
}

/// connect(fd, sockaddr_in, len)：TCP 发起连接（SYN 由 net_poll 发出）。
fn sys_connect(fd: u64, addr: u64, _len: u64) -> u64 {
    let mut sa = [0u8; 16];
    // SAFETY: addr 为用户态 sockaddr_in（16 字节可读）。
    unsafe { core::ptr::copy_nonoverlapping(addr as *const u8, sa.as_mut_ptr(), 16) };
    let port = u16::from_be_bytes([sa[2], sa[3]]);
    let ip = [sa[4], sa[5], sa[6], sa[7]];
    crate::socket::tcp_connect(fd as usize, ip, port) as u64
}

/// sendto(fd, buf, len, flags, dest, dlen)：TCP 走 send（dest 可空），UDP 读目标端口。
fn sys_sendto(fd: u64, buf: u64, len: u64, _flags: u64, dest: u64, dlen: u64) -> u64 {
    let n = core::cmp::min(len, 1472) as usize;
    let mut data = alloc::vec![0u8; n];
    // SAFETY: buf 为用户态可读 n 字节。
    unsafe { core::ptr::copy_nonoverlapping(buf as *const u8, data.as_mut_ptr(), n) };
    if fd >= crate::socket::TCP_FD_BASE as u64 {
        return crate::socket::tcp_send(fd as usize, &data) as u64;
    }
    if dlen < 8 {
        return (-22i64) as u64; // EINVAL
    }
    let mut sa = [0u8; 16];
    // SAFETY: dest 为用户态 sockaddr_in（16 字节可读）。
    unsafe { core::ptr::copy_nonoverlapping(dest as *const u8, sa.as_mut_ptr(), 16) };
    let port = u16::from_be_bytes([sa[2], sa[3]]);
    crate::socket::udp_sendto(fd as usize, &data, port) as u64
}

/// recvfrom(fd, buf, len, flags, src, slen)：非阻塞取接收缓冲。
fn sys_recvfrom(fd: u64, buf: u64, len: u64, _flags: u64, _src: u64, _slen: u64) -> u64 {
    let n = core::cmp::min(len, 4096) as usize;
    if fd >= crate::socket::TCP_FD_BASE as u64 {
        crate::socket::tcp_recv(fd as usize, buf as *mut u8, n) as u64
    } else {
        crate::socket::udp_recvfrom(fd as usize, buf as *mut u8, n) as u64
    }
}

/// epoll_create(size)：创建 epoll 实例（M5-切片5）。
fn sys_epoll_create(size: u64) -> u64 {
    crate::socket::epoll_create(size as usize) as u64
}

/// epoll_ctl(epfd, op, fd, event)：ADD/DEL/MOD。
fn sys_epoll_ctl(epfd: u64, op: u64, fd: u64, event: u64) -> u64 {
    let mut ev = [0u8; 12];
    if event != 0 {
        // SAFETY: event 为用户态 epoll_event（12 字节可读）。
        unsafe { core::ptr::copy_nonoverlapping(event as *const u8, ev.as_mut_ptr(), 12) };
    }
    let events = u32::from_le_bytes([ev[0], ev[1], ev[2], ev[3]]);
    crate::socket::epoll_ctl(epfd as usize, op as u32, fd as usize, events) as u64
}

/// epoll_wait(epfd, events, maxevents, timeout)：非阻塞轮询就绪项。
fn sys_epoll_wait(epfd: u64, events: u64, maxevents: u64, _timeout: u64) -> u64 {
    crate::socket::epoll_wait(
        epfd as usize,
        events as *mut u8,
        core::cmp::min(maxevents, 64) as usize,
    ) as u64
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
fn sys_mount(source: u64, target: u64, fstype: u64, _flags: u64, _data: u64) -> u64 {
    let src = match copy_path(source) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let tgt = match copy_path(target) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let mut ft = [0u8; 16];
    if fstype != 0 {
        // SAFETY: fstype 为用户态 NUL 结尾字符串。
        let n = unsafe { copy_cstr_from_user(fstype, &mut ft) };
        let _ = n;
    }
    let fstype_str = core::str::from_utf8(&ft[..ft.iter().position(|&b| b == 0).unwrap_or(0)])
        .unwrap_or("");
    let r = if fstype_str == "overlay" {
        crate::fs::mount_overlay(&src, &tgt)
    } else if fstype_str == "proc" {
        crate::fs::mount_fs_proc(&tgt)
    } else {
        crate::fs::mount_fs(&tgt)
    };
    match r {
        Ok(()) => 0,
        Err(e) => e as u64,
    }
}

/// getpid()：返回当前任务 id（M3 切片：固定 1）。
fn sys_getpid() -> u64 {
    crate::task::current_pid() as u64
}

/// waitpid(pid, status, options)：非阻塞——子已 Exited 则回收并返回 pid，否则 0。
fn sys_waitpid(pid: u64, _status: u64, _options: u64) -> u64 {
    crate::task::waitpid_nb(pid as usize) as u64
}

/// uname(buf)：填 struct utsname（6×65 字节域）——uts namespace 隔离展示。
fn sys_uname(buf: u64) -> u64 {
    let mut u = [0u8; 390];
    let put = |dst: &mut [u8; 390], off: usize, s: &[u8]| {
        let n = core::cmp::min(s.len(), 64);
        dst[off..off + n].copy_from_slice(&s[..n]);
    };
    put(&mut u, 0, b"Shanshui-guanxin");
    put(&mut u, 65, crate::task::gethostname());
    put(&mut u, 130, b"0.3.0");
    put(&mut u, 195, b"M6-uts");
    put(&mut u, 260, b"x86_64");
    put(&mut u, 325, b"");
    // SAFETY: buf 为用户态地址（390 字节可写）。
    unsafe { core::ptr::copy_nonoverlapping(u.as_ptr(), buf as *mut u8, 390) };
    0
}

/// sethostname(name, len)：设置当前 uts ns 的 hostname。
fn sys_sethostname(name: u64, len: u64) -> u64 {
    let n = core::cmp::min(len as usize, 31);
    // SAFETY: name 为用户态可读 n 字节。
    let data = unsafe { core::slice::from_raw_parts(name as *const u8, n) };
    crate::task::sethostname(data) as u64
}

/// cgroup 统计：向 buf 写 { pids: u64, mem: u64 }（当前任务所在 cgroup）。
fn sys_cgroup_stat(buf: u64) -> u64 {
    let (pids, mem) = crate::task::cgroup_stat(crate::task::current_cgroup());
    let mut tmp = [0u8; 16];
    tmp[0..8].copy_from_slice(&pids.to_le_bytes());
    tmp[8..16].copy_from_slice(&mem.to_le_bytes());
    // SAFETY: buf 为用户态地址（16 字节可写）。
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, 16) };
    0
}

/// nat_add(proto, listen_port, container_port)：添加网关端口映射规则（M8-切片2）。
fn sys_nat_add(proto: u64, listen: u64, container: u64) -> u64 {
    crate::net::nat_add(proto as u8, listen as u16, container as u16) as u64
}

/// conntrack 统计：向 buf 写 { entries: u64, hits: u64 }（当前会话）。
fn sys_ct_stat(buf: u64) -> u64 {
    let (entries, hits) = crate::net::ct_stats();
    let mut tmp = [0u8; 16];
    tmp[0..8].copy_from_slice(&entries.to_le_bytes());
    tmp[8..16].copy_from_slice(&hits.to_le_bytes());
    // SAFETY: buf 为用户态地址（16 字节可写）。
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, 16) };
    0
}

/// fw_add(proto, dport, action)：添加防火墙规则（M8-切片3）。
fn sys_fw_add(proto: u64, dport: u64, action: u64) -> u64 {
    crate::net::fw_add(proto as u8, dport as u16, action as u8) as u64
}

/// fw_del(proto, dport)：删除防火墙规则。
fn sys_fw_del(proto: u64, dport: u64) -> u64 {
    crate::net::fw_del(proto as u8, dport as u16) as u64
}

/// 防火墙统计：向 buf 写 { rules: u64, drops: u64 }。
fn sys_fw_stat(buf: u64) -> u64 {
    let (rules, drops) = crate::net::fw_stats();
    let mut tmp = [0u8; 16];
    tmp[0..8].copy_from_slice(&rules.to_le_bytes());
    tmp[8..16].copy_from_slice(&drops.to_le_bytes());
    // SAFETY: buf 为用户态地址（16 字节可写）。
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, 16) };
    0
}

/// blk_rw(lba, buf, len, is_write)：BIO 扇区读写（M10-切片1）。
fn sys_blk_rw(lba: u64, buf: u64, len: u64, is_write: u64) -> u64 {
    if len as usize > crate::block::SECTOR_SIZE {
        return (-22i64) as u64; // EINVAL
    }
    if is_write != 0 {
        // SAFETY: buf 为用户态可读 len 字节（syscall 路径用户页已映射）。
        let data = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
        crate::block::bio_write(lba, data) as u64
    } else {
        // SAFETY: buf 为用户态可写 len 字节。
        let dst = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
        crate::block::bio_read(lba, dst) as u64
    }
}

/// 从用户态取名字（原样，不做 cwd 解析——ext4-lite 文件名为纯键）。
fn copy_name(path: u64) -> Result<alloc::string::String, u64> {
    let mut nb = [0u8; 64];
    // SAFETY: path 为用户态 NUL 结尾字符串。
    let n = unsafe { copy_cstr_from_user(path, &mut nb) };
    // SAFETY: 文件名为 ASCII。
    Ok(alloc::string::String::from(unsafe {
        core::str::from_utf8_unchecked(&nb[..n])
    }))
}

/// blkfs(op, name, buf, len, offset)：ext4-lite 文件系统操作（M10-切片2）。
/// op：0=init 1=create 2=write 3=read 4=unlink 5=sync 6=drop-cache 7=list。
fn sys_blkfs(op: u64, name: u64, buf: u64, len: u64, offset: u64) -> u64 {
    match op {
        0 => crate::ext4::blkfs_init() as u64,
        1 => match copy_name(name) {
            Ok(n) => crate::ext4::blkfs_create(&n) as u64,
            Err(e) => e,
        },
        2 => {
            let nm = match copy_name(name) {
                Ok(n) => n,
                Err(e) => return e,
            };
            // SAFETY: buf 为用户态可读 len 字节。
            let data = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
            crate::ext4::blkfs_write(&nm, data, offset as usize) as u64
        }
        3 => {
            let nm = match copy_name(name) {
                Ok(n) => n,
                Err(e) => return e,
            };
            // SAFETY: buf 为用户态可写 len 字节。
            let dst = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
            crate::ext4::blkfs_read(&nm, dst, offset as usize) as u64
        }
        4 => match copy_name(name) {
            Ok(n) => crate::ext4::blkfs_unlink(&n) as u64,
            Err(e) => e,
        },
        5 => crate::ext4::blkfs_sync() as u64,
        6 => {
            crate::ext4::blkfs_drop();
            0
        }
        7 => {
            // SAFETY: buf 为用户态可写 len 字节。
            let dst = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
            crate::ext4::blkfs_list(dst) as u64
        }
        _ => (-22i64) as u64, // EINVAL
    }
}

/// exit(code)：用户态进程退出——M3 切片先打印并停机（真正的 exit 待进程模型完善）。
fn sys_exit(code: u64) -> u64 {
    crate::println!("[syscall] task exit({code})");
    // M6-切片1：非 init 任务走 task::exit（置 Exited，父可 waitpid 回收）；init（task 0）退出则停机。
    if crate::task::current_id() != 0 {
        crate::task::exit();
    }
    // SAFETY: hlt 特权指令，用户态退出后停机（init）。
    unsafe { asm!("hlt", options(nomem, nostack)) };
    0 // 不可达
}

/// 供启动日志确认 syscall 就绪。
pub fn info() -> &'static str {
    "syscall(msr/table) ready"
}
