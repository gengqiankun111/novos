//! M5-切片3/4：UDP + TCP socket。
//!
//! 简化设计：
//! - fd 空间：文件 <100；UDP 100 起（表索引 +100）；TCP 200 起（表索引 +200）；
//! - UDP：每 socket 一个接收缓冲（Vec<Vec<u8>>，按数据报弹出）；
//! - TCP：连接表按 fd 索引，状态机（Listen/SynReceived/SynSent/Established/FinWait/
//!   CloseWait/LastAck）；收发走 `net_poll` 轮询刷新（`tcp_drain_tx`）；
//! - 非阻塞：无数据返回 0（用户态自旋等待）。
//!
//! 错误码（Linux）：EBADF=9, EADDRINUSE=98, EINVAL=22, EAFNOSUPPORT=-1。

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

// ---- UDP（M5-切片3）----

pub struct UdpSocket {
    pub fd: usize,
    pub local_port: u16,
    pub bound: bool,
    /// 接收队列：每元素一个完整 UDP 数据报（recvfrom 按数据报弹出）。
    pub recv: Vec<Vec<u8>>,
}

static SOCKETS: spin::Lazy<Mutex<Vec<UdpSocket>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

/// socket(AF_INET, SOCK_DGRAM)：创建 UDP socket。
pub fn udp_socket() -> i64 {
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
pub fn udp_bind(fd: usize, port: u16) -> i64 {
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
pub fn udp_sendto(fd: usize, data: &[u8], dst_port: u16) -> i64 {
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
pub fn udp_recvfrom(fd: usize, buf: *mut u8, len: usize) -> i64 {
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
pub fn udp_close(fd: usize) -> i64 {
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

// ---- TCP（M5-切片4）----

/// TCP fd 空间起点（UDP 100 起、文件 <100）。
pub const TCP_FD_BASE: usize = 200;

/// TCP 标志位。
pub const TCP_FIN: u8 = 0x01;
pub const TCP_SYN: u8 = 0x02;
pub const TCP_RST: u8 = 0x04;
pub const TCP_PSH: u8 = 0x08;
pub const TCP_ACK: u8 = 0x10;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynReceived,
    SynSent,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
}

/// 一个 TCP 连接（fd 直接映射为表索引：fd = TCP_FD_BASE + idx）。
pub struct TcpConn {
    pub state: TcpState,
    pub local_port: u16,
    pub peer_ip: [u8; 4],
    pub peer_port: u16,
    /// 下一个待发 seq（初始 = iss+1）。
    pub snd_nxt: u32,
    /// 期望收到的 seq。
    pub rcv_nxt: u32,
    /// 用户待读数据（字节流）。
    pub rx: Vec<u8>,
    /// 用户待发数据（net_poll 时 flush）。
    pub tx: Vec<u8>,
    pub fin_sent: bool,
    pub sent_syn: bool,
    pub dead: bool,
}

/// 监听 socket：fd + 端口 + 待 accept 的连接索引（按到达顺序）。
pub struct TcpListener {
    pub fd: usize,
    pub local_port: u16,
    pub pending: Vec<usize>,
}

/// 待发送的 TCP 段（由 net 层组帧）。
pub struct TcpSeg {
    pub dst_ip: [u8; 4],
    pub dst_port: u16,
    pub src_port: u16,
    pub flags: u8,
    pub seq: u32,
    pub ack: u32,
    pub data: Vec<u8>,
}

static TCP_CONNS: spin::Lazy<Mutex<Vec<Option<TcpConn>>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));
static TCP_LISTENERS: spin::Lazy<Mutex<Vec<TcpListener>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));
static ISN: spin::Lazy<Mutex<u32>> = spin::Lazy::new(|| Mutex::new(0x4000_0000));

/// TCP fd 判断（供 syscall 分派）。
pub fn is_tcp_fd(fd: usize) -> bool {
    fd >= TCP_FD_BASE
}

fn alloc_slot(c: &mut Vec<Option<TcpConn>>) -> usize {
    if let Some(i) = c.iter().position(|x| x.is_none()) {
        i
    } else {
        c.push(None);
        c.len() - 1
    }
}

fn idx_of(fd: usize) -> usize {
    fd - TCP_FD_BASE
}

/// socket(AF_INET, SOCK_STREAM)：创建 TCP socket（初始 Closed）。
pub fn tcp_socket() -> i64 {
    let mut c = TCP_CONNS.lock();
    let idx = alloc_slot(&mut c);
    c[idx] = Some(TcpConn {
        state: TcpState::Closed,
        local_port: 0,
        peer_ip: [0; 4],
        peer_port: 0,
        snd_nxt: 0,
        rcv_nxt: 0,
        rx: vec![],
        tx: vec![],
        fin_sent: false,
        sent_syn: false,
        dead: false,
    });
    (TCP_FD_BASE + idx) as i64
}

/// bind(fd, port)：绑定本地端口（UDP/TCP 表共用端口空间检查）。
pub fn tcp_bind(fd: usize, port: u16) -> i64 {
    let c = TCP_CONNS.lock();
    if c.iter().flatten().any(|x| x.local_port == port && x.state != TcpState::Closed) {
        return -98; // EADDRINUSE
    }
    drop(c);
    if SOCKETS.lock().iter().any(|x| x.bound && x.local_port == port) {
        return -98;
    }
    let mut c = TCP_CONNS.lock();
    match c.get_mut(idx_of(fd)).and_then(|s| s.as_mut()) {
        Some(x) => {
            x.local_port = port;
            0
        }
        None => -9,
    }
}

/// listen(fd, backlog)：进入监听态并注册监听表。
pub fn tcp_listen(fd: usize, _backlog: usize) -> i64 {
    let port = {
        let c = TCP_CONNS.lock();
        match c.get(idx_of(fd)).and_then(|s| s.as_ref()) {
            Some(x) if x.state == TcpState::Closed => x.local_port,
            _ => return -9,
        }
    };
    if port == 0 {
        return -22; // EINVAL：未 bind
    }
    {
        let mut c = TCP_CONNS.lock();
        if let Some(x) = c.get_mut(idx_of(fd)).and_then(|s| s.as_mut()) {
            x.state = TcpState::Listen;
        }
    }
    TCP_LISTENERS.lock().push(TcpListener {
        fd,
        local_port: port,
        pending: Vec::new(),
    });
    0
}

/// accept(fd)：非阻塞返回第一个已 Established 的连接 fd；无则 0。
pub fn tcp_accept(fd: usize) -> i64 {
    let pend = {
        let l = TCP_LISTENERS.lock();
        match l.iter().find(|x| x.fd == fd) {
            Some(x) => x.pending.clone(),
            None => return -9,
        }
    };
    let mut matched: Option<usize> = None;
    {
        let c = TCP_CONNS.lock();
        for &pidx in &pend {
            if let Some(Some(conn)) = c.get(pidx) {
                if conn.state == TcpState::Established {
                    matched = Some(pidx);
                    break;
                }
            }
        }
    }
    if let Some(pidx) = matched {
        TCP_LISTENERS
            .lock()
            .iter_mut()
            .find(|x| x.fd == fd)
            .map(|x| x.pending.retain(|&p| p != pidx));
        (TCP_FD_BASE + pidx) as i64
    } else {
        0
    }
}

/// connect(fd, ip, port)：发起 SYN（实际发送由 net_poll 的 drain 完成）。
pub fn tcp_connect(fd: usize, peer_ip: [u8; 4], peer_port: u16) -> i64 {
    let mut c = TCP_CONNS.lock();
    match c.get_mut(idx_of(fd)).and_then(|s| s.as_mut()) {
        Some(x) if x.state == TcpState::Closed => {
            if x.local_port == 0 {
                return -22; // EINVAL：未 bind（简化：connect 前须 bind 固定端口）
            }
            x.peer_ip = peer_ip;
            x.peer_port = peer_port;
            let iss = {
                let mut n = ISN.lock();
                *n = n.wrapping_add(0x1000);
                *n
            };
            x.snd_nxt = iss.wrapping_add(1);
            x.rcv_nxt = 0;
            x.sent_syn = false;
            x.state = TcpState::SynSent;
            0
        }
        _ => -9,
    }
}

/// send(fd, data)：数据入 tx，由 net_poll 实际发出。
pub fn tcp_send(fd: usize, data: &[u8]) -> i64 {
    let mut c = TCP_CONNS.lock();
    match c.get_mut(idx_of(fd)).and_then(|s| s.as_mut()) {
        Some(x) if x.state == TcpState::Established => {
            x.tx.extend_from_slice(data);
            data.len() as i64
        }
        Some(_) => -1, // 非 Established（EPERM）
        None => -9,
    }
}

/// recv(fd, buf, len)：非阻塞读取 rx 字节流。
pub fn tcp_recv(fd: usize, buf: *mut u8, len: usize) -> i64 {
    let mut c = TCP_CONNS.lock();
    match c.get_mut(idx_of(fd)).and_then(|s| s.as_mut()) {
        Some(x) if x.state == TcpState::Established || x.state == TcpState::CloseWait => {
            if x.rx.is_empty() {
                return 0;
            }
            let n = core::cmp::min(x.rx.len(), len);
            // SAFETY: buf 为用户态可写 len 字节。
            unsafe { core::ptr::copy_nonoverlapping(x.rx.as_ptr(), buf, n) };
            x.rx.drain(..n);
            n as i64
        }
        _ => -9,
    }
}

/// close(fd)：Established 走 FIN；其余直接释放。
pub fn tcp_close(fd: usize) -> i64 {
    let mut c = TCP_CONNS.lock();
    let idx = idx_of(fd);
    match c.get_mut(idx).and_then(|s| s.as_mut()) {
        Some(x) => {
            match x.state {
                TcpState::Closed | TcpState::Listen => {
                    c[idx] = None;
                }
                TcpState::Established => x.state = TcpState::FinWait1, // drain 发 FIN
                TcpState::CloseWait => x.state = TcpState::LastAck,    // drain 发 FIN
                TcpState::SynReceived | TcpState::SynSent => {
                    x.state = TcpState::Closed;
                    x.dead = true;
                }
                _ => x.state = TcpState::Closed, // FinWait1/2/LastAck：置死待回收
            }
            drop(c);
            TCP_LISTENERS.lock().retain(|l| l.fd != fd);
            0
        }
        None => -9,
    }
}

fn seg(dst_ip: [u8; 4], dst_port: u16, src_port: u16, flags: u8, seq: u32, ack: u32) -> TcpSeg {
    TcpSeg {
        dst_ip,
        dst_port,
        src_port,
        flags,
        seq,
        ack,
        data: vec![],
    }
}

/// 回收 dead 连接并清理监听 pending。
fn tcp_gc(c: &mut Vec<Option<TcpConn>>) {
    for (i, slot) in c.iter_mut().enumerate() {
        if let Some(x) = slot {
            if x.dead {
                *slot = None;
                // 从所有监听 pending 中移除
                for l in TCP_LISTENERS.lock().iter_mut() {
                    l.pending.retain(|&p| p != i);
                }
            }
        }
    }
}

/// net_poll 调用：处理入站 TCP 段，返回待发段。
pub fn tcp_receive(
    src_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    flags: u8,
    seq: u32,
    ack: u32,
    data: &[u8],
) -> Vec<TcpSeg> {
    let mut out = vec![];
    let mut c = TCP_CONNS.lock();

    // 1) 已有连接（4 元组匹配）
    let found = c.iter().enumerate().find_map(|(i, slot)| match slot {
        Some(x)
            if x.state != TcpState::Closed
                && x.state != TcpState::Listen
                && x.local_port == dst_port
                && x.peer_port == src_port
                && x.peer_ip == src_ip =>
        {
            Some(i)
        }
        _ => None,
    });
    if let Some(i) = found {
        let state = c[i].as_ref().unwrap().state;
        match state {
            TcpState::SynReceived => {
                if flags & TCP_ACK != 0 {
                    c[i].as_mut().unwrap().state = TcpState::Established;
                }
            }
            TcpState::SynSent => {
                if flags & TCP_SYN != 0 && flags & TCP_ACK != 0 {
                    let x = c[i].as_mut().unwrap();
                    x.rcv_nxt = seq.wrapping_add(1);
                    x.state = TcpState::Established;
                    out.push(seg(src_ip, src_port, dst_port, TCP_ACK, x.snd_nxt, x.rcv_nxt));
                }
            }
            TcpState::Established => {
                if flags & TCP_RST != 0 {
                    let x = c[i].as_mut().unwrap();
                    x.dead = true;
                    x.state = TcpState::Closed;
                } else {
                    // 按序接收数据
                    if !data.is_empty() && seq == c[i].as_ref().unwrap().rcv_nxt {
                        let x = c[i].as_mut().unwrap();
                        x.rx.extend_from_slice(data);
                        x.rcv_nxt = x.rcv_nxt.wrapping_add(data.len() as u32);
                        out.push(seg(src_ip, src_port, dst_port, TCP_ACK, x.snd_nxt, x.rcv_nxt));
                    }
                    if flags & TCP_FIN != 0 {
                        let x = c[i].as_mut().unwrap();
                        x.rcv_nxt = x.rcv_nxt.wrapping_add(1);
                        x.state = TcpState::CloseWait;
                        out.push(seg(src_ip, src_port, dst_port, TCP_ACK, x.snd_nxt, x.rcv_nxt));
                    }
                }
            }
            TcpState::CloseWait => {}
            TcpState::FinWait1 => {
                if flags & TCP_ACK != 0 {
                    c[i].as_mut().unwrap().state = TcpState::FinWait2;
                }
                if flags & TCP_FIN != 0 {
                    let x = c[i].as_mut().unwrap();
                    x.rcv_nxt = x.rcv_nxt.wrapping_add(1);
                    x.state = TcpState::Closed;
                    x.dead = true;
                    out.push(seg(src_ip, src_port, dst_port, TCP_ACK, x.snd_nxt, x.rcv_nxt));
                }
            }
            TcpState::FinWait2 => {
                if flags & TCP_FIN != 0 {
                    let x = c[i].as_mut().unwrap();
                    x.rcv_nxt = x.rcv_nxt.wrapping_add(1);
                    x.state = TcpState::Closed;
                    x.dead = true;
                    out.push(seg(src_ip, src_port, dst_port, TCP_ACK, x.snd_nxt, x.rcv_nxt));
                }
            }
            TcpState::LastAck => {
                if flags & TCP_ACK != 0 {
                    let x = c[i].as_mut().unwrap();
                    x.dead = true;
                    x.state = TcpState::Closed;
                }
            }
            _ => {}
        }
        tcp_gc(&mut c);
        return out;
    }

    // 2) SYN → 找监听者，新建连接
    if flags & TCP_SYN != 0 && flags & TCP_ACK == 0 {
        let lis = {
            let l = TCP_LISTENERS.lock();
            l.iter().find(|x| x.local_port == dst_port).map(|x| (x.fd, x.local_port))
        };
        if let Some((lfd, lport)) = lis {
            let iss = {
                let mut n = ISN.lock();
                *n = n.wrapping_add(0x1000);
                *n
            };
            let nidx = alloc_slot(&mut c);
            c[nidx] = Some(TcpConn {
                state: TcpState::SynReceived,
                local_port: lport,
                peer_ip: src_ip,
                peer_port: src_port,
                snd_nxt: iss.wrapping_add(1),
                rcv_nxt: seq.wrapping_add(1),
                rx: vec![],
                tx: vec![],
                fin_sent: false,
                sent_syn: false,
                dead: false,
            });
            // 挂到监听 pending（SYN_RCVD 阶段即入列，Established 后 accept 可取）
            for l in TCP_LISTENERS.lock().iter_mut() {
                if l.fd == lfd {
                    l.pending.push(nidx);
                }
            }
            out.push(seg(src_ip, src_port, dst_port, TCP_SYN | TCP_ACK, iss, seq.wrapping_add(1)));
        }
    }
    out
}

/// net_poll 调用：flush 所有连接待发数据 / SYN / FIN。
pub fn tcp_drain_tx() -> Vec<TcpSeg> {
    let mut out = vec![];
    let mut c = TCP_CONNS.lock();
    for (i, slot) in c.iter_mut().enumerate() {
        if let Some(x) = slot {
            match x.state {
                TcpState::SynSent => {
                    if !x.sent_syn {
                        x.sent_syn = true;
                        let seq = x.snd_nxt.wrapping_sub(1); // iss
                        out.push(seg(x.peer_ip, x.peer_port, x.local_port, TCP_SYN, seq, 0));
                    }
                }
                TcpState::Established => {
                    if !x.tx.is_empty() {
                        let seq = x.snd_nxt;
                        x.snd_nxt = x.snd_nxt.wrapping_add(x.tx.len() as u32);
                        out.push(TcpSeg {
                            dst_ip: x.peer_ip,
                            dst_port: x.peer_port,
                            src_port: x.local_port,
                            flags: TCP_ACK | TCP_PSH,
                            seq,
                            ack: x.rcv_nxt,
                            data: core::mem::take(&mut x.tx),
                        });
                    }
                }
                TcpState::FinWait1 | TcpState::LastAck => {
                    if !x.fin_sent {
                        x.fin_sent = true;
                        let seq = x.snd_nxt;
                        x.snd_nxt = x.snd_nxt.wrapping_add(1);
                        out.push(seg(x.peer_ip, x.peer_port, x.local_port, TCP_ACK | TCP_FIN, seq, x.rcv_nxt));
                    }
                }
                _ => {}
            }
        }
        if let Some(x) = slot {
            if x.dead {
                *slot = None;
                for l in TCP_LISTENERS.lock().iter_mut() {
                    l.pending.retain(|&p| p != i);
                }
            }
        }
    }
    out
}

// ---- epoll（M5-切片5）----

/// epoll fd 空间起点（UDP 100、TCP 200、epoll 300）。
pub const EPOLL_FD_BASE: usize = 300;

/// epoll_ctl 操作。
pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;

/// 关注事件（EPOLLIN=0x1，其他后续扩展）。
pub const EPOLLIN: u32 = 0x1;

pub struct EpollItem {
    pub fd: usize,
    pub events: u32,
}

pub struct EpollInst {
    pub fd: usize,
    pub items: Vec<EpollItem>,
}

static EPOLLS: spin::Lazy<Mutex<Vec<EpollInst>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

pub fn is_epoll_fd(fd: usize) -> bool {
    fd >= EPOLL_FD_BASE
}

/// epoll_create(size)：创建 epoll 实例。
pub fn epoll_create(_size: usize) -> i64 {
    let mut e = EPOLLS.lock();
    let fd = EPOLL_FD_BASE + e.len();
    e.push(EpollInst {
        fd,
        items: Vec::new(),
    });
    fd as i64
}

/// epoll_ctl(epfd, op, fd, events)：ADD/DEL/MOD 关注项。
pub fn epoll_ctl(epfd: usize, op: u32, fd: usize, events: u32) -> i64 {
    let mut e = EPOLLS.lock();
    match e.iter_mut().find(|x| x.fd == epfd) {
        None => -9, // EBADF
        Some(inst) => {
            let pos = inst.items.iter().position(|x| x.fd == fd);
            match op {
                EPOLL_CTL_ADD => {
                    if pos.is_some() {
                        return -17; // EEXIST
                    }
                    inst.items.push(EpollItem { fd, events });
                    0
                }
                EPOLL_CTL_DEL => match pos {
                    Some(i) => {
                        inst.items.remove(i);
                        0
                    }
                    None => -9,
                },
                EPOLL_CTL_MOD => match pos {
                    Some(i) => {
                        inst.items[i].events = events;
                        0
                    }
                    None => -9,
                },
                _ => -22, // EINVAL
            }
        }
    }
}

/// TCP fd 就绪：监听有已建立连接待 accept；连接态有数据可读。
pub fn tcp_ready(fd: usize) -> bool {
    let c = TCP_CONNS.lock();
    match c.get(idx_of(fd)).and_then(|s| s.as_ref()) {
        Some(x) => match x.state {
            TcpState::Listen => {
                let l = TCP_LISTENERS.lock();
                l.iter().find(|lis| lis.fd == fd).map_or(false, |lis| {
                    lis.pending.iter().any(|&p| {
                        c.get(p)
                            .and_then(|s| s.as_ref())
                            .map_or(false, |cc| cc.state == TcpState::Established)
                    })
                })
            }
            TcpState::Established | TcpState::CloseWait => !x.rx.is_empty(),
            _ => false,
        },
        None => false,
    }
}

/// UDP fd 就绪：接收队列非空。
pub fn udp_ready(fd: usize) -> bool {
    let s = SOCKETS.lock();
    s.iter()
        .find(|x| x.fd == fd)
        .map_or(false, |x| !x.recv.is_empty())
}

fn epoll_item_ready(fd: usize) -> bool {
    if fd >= crate::timer::TIMER_FD_BASE {
        // M13-11：timerfd 到期可读
        crate::timer::timer_ready(fd)
    } else if fd >= TCP_FD_BASE {
        tcp_ready(fd)
    } else if fd >= 100 {
        udp_ready(fd)
    } else {
        false
    }
}

/// epoll_wait(epfd, events, maxevents, timeout_ms)：阻塞等待就绪项。
/// 每个 epoll_event 为 { events: u32, data: u64 }（12 字节）。
/// timeout_ms=0 非阻塞；>0 时在 deadline 前反复检查，期间 hlt 让出（tick 唤醒）。
pub fn epoll_wait(epfd: usize, events: *mut u8, maxevents: usize, timeout_ms: u64) -> i64 {
    // PIT 100Hz：10ms/tick，超时换算成 tick 截止时刻。
    let deadline = crate::task::ticks() + timeout_ms / 10;
    loop {
        {
            let e = EPOLLS.lock();
            let inst = match e.iter().find(|x| x.fd == epfd) {
                Some(x) => x,
                None => return -9,
            };
            let mut n = 0usize;
            for item in &inst.items {
                if n >= maxevents {
                    break;
                }
                if epoll_item_ready(item.fd) {
                    // SAFETY: events 为用户态可写 maxevents*12 字节。
                    unsafe {
                        let base = events.add(n * 12);
                        core::ptr::write_volatile(base as *mut u32, item.events);
                        core::ptr::write_volatile(base.add(4) as *mut u32, 0); // pad
                        core::ptr::write_volatile(base.add(8) as *mut u64, 0); // data
                    }
                    n += 1;
                }
            }
            if n > 0 {
                return n as i64;
            }
        } // 释放 EPOLLS 锁后再休眠
        if timeout_ms == 0 || crate::task::ticks() >= deadline {
            return 0;
        }
        crate::task::sleep_deadline(deadline);
    }
}

/// close epoll 实例。
pub fn epoll_close(fd: usize) -> i64 {
    let mut e = EPOLLS.lock();
    match e.iter().position(|x| x.fd == fd) {
        Some(i) => {
            e.remove(i);
            0
        }
        None => -9,
    }
}
