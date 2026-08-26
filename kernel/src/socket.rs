//! M5-切片3：UDP socket 基础。
//!
//! 简化设计：
//! - 独立 fd 空间（100 起，与文件 fd 表分离）；
//! - 每 socket 一个接收缓冲（Vec），`net_poll` 收到 UDP 后按目的端口投递；
//! - sendto 目标固定网关（QEMU user 模式 10.0.2.2）；
//! - recvfrom 非阻塞：无数据返回 0。
//!
//! 错误码（Linux）：EBADF=9, EADDRINUSE=98, EINVAL=22。

use alloc::vec::Vec;
use spin::Mutex;

pub struct UdpSocket {
    pub fd: usize,
    pub local_port: u16,
    pub bound: bool,
    /// 接收队列：每元素一个完整 UDP 数据报（recvfrom 按数据报弹出）。
    pub recv: Vec<Vec<u8>>,
}

static SOCKETS: spin::Lazy<Mutex<Vec<UdpSocket>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

/// socket(AF_INET, SOCK_DGRAM)：创建 UDP socket。
pub fn socket_create() -> i64 {
    let mut s = SOCKETS.lock();
    let fd = 100 + s.len();
    s.push(UdpSocket {
        fd,
        local_port: 0,
        bound: false,
        recv: Vec::new(),
    });
    fd as i64
}

/// bind(fd, port)：绑定本地端口（占用检查）。
pub fn socket_bind(fd: usize, port: u16) -> i64 {
    let mut s = SOCKETS.lock();
    if s.iter().any(|x| x.bound && x.local_port == port) {
        return -98; // EADDRINUSE
    }
    match s.iter_mut().find(|x| x.fd == fd) {
        Some(x) => {
            x.local_port = port;
            x.bound = true;
            0
        }
        None => -9, // EBADF
    }
}

/// sendto(fd, data, dst_port)：发 UDP 到网关端口。
pub fn socket_sendto(fd: usize, data: &[u8], dst_port: u16) -> i64 {
    let src_port = {
        let s = SOCKETS.lock();
        match s.iter().find(|x| x.fd == fd) {
            Some(x) => x.local_port,
            None => return -9, // EBADF
        }
    };
    crate::net::send_udp(crate::net::GATEWAY_IP, dst_port, src_port, data);
    data.len() as i64
}

/// recvfrom(fd, buf, len)：非阻塞弹出一个 UDP 数据报（不足 len 则整体拷贝）。
pub fn socket_recvfrom(fd: usize, buf: *mut u8, len: usize) -> i64 {
    let mut s = SOCKETS.lock();
    match s.iter_mut().find(|x| x.fd == fd) {
        Some(x) => {
            if x.recv.is_empty() {
                return 0; // 非阻塞：无数据
            }
            let msg = &x.recv[0];
            let n = core::cmp::min(msg.len(), len);
            // SAFETY: buf 为用户态可写 len 字节。
            unsafe { core::ptr::copy_nonoverlapping(msg.as_ptr(), buf, n) };
            x.recv.remove(0);
            n as i64
        }
        None => -9, // EBADF
    }
}

/// close(fd)：关闭 socket。
pub fn socket_close(fd: usize) -> i64 {
    let mut s = SOCKETS.lock();
    match s.iter().position(|x| x.fd == fd) {
        Some(i) => {
            s.remove(i);
            0
        }
        None => -9, // EBADF
    }
}

/// net_poll 投递 UDP 数据到绑定端口的 socket（每包一个数据报，总缓冲上限 8KB 防失控）。
pub fn udp_deliver(port: u16, data: &[u8]) {
    let mut s = SOCKETS.lock();
    if let Some(x) = s.iter_mut().find(|x| x.bound && x.local_port == port) {
        let total: usize = x.recv.iter().map(|m| m.len()).sum();
        if total < 8192 {
            x.recv.push(data.to_vec());
        }
    }
}
