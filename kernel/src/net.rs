//! M5-切片1：virtio-net 驱动（PCI 传统模式）+ 以太网收发 + ARP。
//!
//! 简化设计：
//! - **轮询模式**：不依赖中断（QEMU PCI INTx 走未初始化的 IOAPIC），
//!   收包在 `net_poll()` 中检查 used ring；
//! - 传统 virtio-pci（I/O BAR + vring PFN），QEMU 需 `disable-modern=on`；
//! - tx 同步发送（kick 后轮询 used 完成）；rx 预填 WRITE 描述符后轮询取帧；
//! - rx 描述符固定占 [0, RX_BUFS)，tx 固定占 [RX_BUFS, QUEUE_SIZE)，互不冲突；
//! - 本机 IP 静态 10.0.2.15（QEMU user 网）。

use crate::mm;
use core::arch::asm;

// ---- PCI ----
const PCI_CONFIG_ADDR: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;
const VIRTIO_PCI_VENDOR: u16 = 0x1AF4;
const VIRTIO_PCI_DEVICE_NET: u16 = 0x1000;

// ---- virtio 传统模式配置寄存器（I/O BAR 偏移）----
const REG_HOST_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_NUM: u16 = 0x0C;
const REG_QUEUE_SEL: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
const REG_DEVICE_CFG: u16 = 0x14;

// ---- status 位 ----
const VIRTIO_ACKNOWLEDGE: u8 = 1;
const VIRTIO_DRIVER: u8 = 2;
const VIRTIO_DRIVER_OK: u8 = 4;
const VIRTIO_FEATURES_OK: u8 = 8;

// ---- 特性位 ----
const VIRTIO_NET_F_MAC: u32 = 5;
const VIRTIO_NET_F_STATUS: u32 = 16;
const VIRTIO_NET_F_NO_CSUM: u32 = 17;
const VIRTIO_NET_F_NO_VLAN: u32 = 18;

const RX_BUFS: usize = 32;
const RX_BUF_SIZE: usize = 2048;

/// 向上对齐（a 须为 2 的幂）。
const fn align_up(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

// ---- 以太网 / ARP ----
const ETH_TYPE_ARP: u16 = 0x0806;
const ETH_TYPE_IP: u16 = 0x0800;

/// 本机 IP（QEMU user 模式默认）。
pub const OUR_IP: [u8; 4] = [10, 0, 2, 15];
/// 网关 IP（QEMU user 模式）。
const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];

/// vring 描述符（传统模式）。
#[repr(C)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct UsedElem {
    id: u32,
    len: u32,
}

/// 一个 virtqueue（vring 布局按设备实际 QUEUE_NUM 计算）。
struct Virtq {
    base: usize,
    desc: *mut VringDesc,
    avail_idx: *mut u16,
    avail_ring: *mut u16,
    used_idx: *mut u16,
    used_ring: *mut UsedElem,
    last_used: u16,
    num: u16,
}

impl Virtq {
    /// 读取设备 queue size，分配 vring 并配置 PFN。
    unsafe fn new(queue_index: u16, io: u16) -> Virtq {
        io_write16(io + REG_QUEUE_SEL, queue_index);
        let num = io_read16(io + REG_QUEUE_NUM); // 设备实际 size（QEMU 默认 256）
        assert!(num >= 32 && num <= 1024, "virtq: bad num {num}");
        let n = num as usize;
        // 传统 vring 布局（align=4096）
        let desc_size = n * 16;
        let avail_off = align_up(desc_size, 2);
        let avail_size = 4 + n * 2;
        let used_off = align_up(avail_off + avail_size, 4096);
        let used_size = 4 + n * 8;
        let total = used_off + used_size;
        let order = if total > 16 * 1024 { 3 } else { 2 };
        let base = mm::alloc_pages(order);
        assert!(base != 0, "virtq: alloc failed");
        // SAFETY: 分配的物理页清零。
        unsafe { core::ptr::write_bytes(base as *mut u8, 0, (1usize << (12 + order))) };
        let q = Virtq {
            base,
            desc: base as *mut VringDesc,
            avail_idx: (base + avail_off + 2) as *mut u16, // flags@+0 idx@+2 ring@+4
            avail_ring: (base + avail_off + 4) as *mut u16,
            used_idx: (base + used_off + 2) as *mut u16,
            used_ring: (base + used_off + 4) as *mut UsedElem,
            last_used: 0,
            num,
        };
        io_write32(io + REG_QUEUE_PFN, (base >> 12) as u32);
        crate::println!("virtq: queue {queue_index} num={num} base={base:#x}");
        q
    }

    /// 追加一个描述符 index 到 avail ring 并 kick。
    fn kick(&mut self, desc_index: u16, io: u16, queue_index: u16) {
        // SAFETY: 设备内存 volatile。
        unsafe {
            let ai = core::ptr::read_volatile(self.avail_idx);
            core::ptr::write_volatile(
                self.avail_ring.add((ai % self.num) as usize),
                desc_index,
            );
            core::ptr::write_volatile(self.avail_idx, ai.wrapping_add(1));
        }
        io_write16(io + REG_QUEUE_NOTIFY, queue_index);
    }

    /// 取出一个已完成的描述符 (id, len)；无则 None。
    fn used_pop(&mut self) -> Option<(u16, u32)> {
        // SAFETY: volatile 读 used idx（设备更新）。
        let used = unsafe { core::ptr::read_volatile(self.used_idx) };
        if self.last_used == used {
            return None;
        }
        let idx = self.last_used % self.num;
        // SAFETY: used ring 元素由设备写入。
        let elem = unsafe { core::ptr::read_volatile(self.used_ring.add(idx as usize)) };
        self.last_used = self.last_used.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }
}

/// virtio-net 设备。
pub struct VirtioNet {
    io: u16,
    pub mac: [u8; 6],
    rxq: Virtq,
    txq: Virtq,
    rx_pages: [usize; RX_BUFS],
    tx_next: u16,
}

// 单核轮询驱动：vring 裸指针仅本 CPU 使用，无跨线程共享。
unsafe impl Send for VirtioNet {}

impl VirtioNet {
    /// 发送以太网帧（tx 同步等待完成）。
    fn send_frame(&mut self, dst_mac: [u8; 6], ethertype: u16, payload: &[u8]) {
        // 栈缓冲：virtio_net_hdr(10) + eth(14) + payload(≤1514)
        let mut frame = [0u8; 10 + 14 + 1514];
        frame[10..16].copy_from_slice(&dst_mac);
        frame[16..22].copy_from_slice(&self.mac);
        frame[22..24].copy_from_slice(&ethertype.to_be_bytes());
        frame[24..24 + payload.len()].copy_from_slice(payload);
        let total = 10 + 14 + payload.len();
        // 分配 tx 描述符（范围 [RX_BUFS, QUEUE_SIZE)）
        let d = self.tx_next;
        self.tx_next = if d + 1 >= self.txq.num {
            RX_BUFS as u16
        } else {
            d + 1
        };
        // SAFETY: frame 为内核栈（恒等映射 = 物理地址）。
        unsafe {
            let desc = self.txq.desc.add(d as usize);
            desc.write_volatile(VringDesc {
                addr: frame.as_ptr() as usize as u64,
                len: total as u32,
                flags: 0,
                next: 0,
            });
            self.txq.kick(d, self.io, 1);
        }
        // 同步等待 used 完成
        let mut spins = 0u32;
        while self.txq.used_pop().is_none() {
            spins += 1;
            if spins > 20_000_000 {
                crate::println!("tx: TIMEOUT waiting used (d={d})");
                break;
            }
        }
    }

    /// 处理收到的帧（ARP 应答/请求）。
    fn handle_frame(&mut self, buf: &[u8]) {
        if buf.len() < 14 {
            return;
        }
        let ethertype = u16::from_be_bytes([buf[12], buf[13]]);
        match ethertype {
            ETH_TYPE_ARP => {
                if buf.len() >= 14 + 28 {
                    self.handle_arp(&buf[14..14 + 28]);
                }
            }
            _ => {} // IPv4 解析留 M5-切片2
        }
    }

    /// ARP：请求（op=1）目标为本机 → 回应答；应答（op=2）→ 打印（验证 rx 路径）。
    fn handle_arp(&mut self, arp: &[u8]) {
        let op = u16::from_be_bytes([arp[6], arp[7]]);
        let sender_mac = [arp[8], arp[9], arp[10], arp[11], arp[12], arp[13]];
        let sender_ip = [arp[14], arp[15], arp[16], arp[17]];
        let target_ip = [arp[24], arp[25], arp[26], arp[27]];
        if op == 1 && target_ip == OUR_IP {
            let mut reply = [0u8; 28];
            reply[0..2].copy_from_slice(&1u16.to_be_bytes()); // htype 以太网
            reply[2..4].copy_from_slice(&0x0800u16.to_be_bytes()); // ptype IPv4
            reply[4] = 6; // hlen
            reply[5] = 4; // plen
            reply[6..8].copy_from_slice(&2u16.to_be_bytes()); // op=reply
            reply[8..14].copy_from_slice(&self.mac);
            reply[14..18].copy_from_slice(&OUR_IP);
            reply[18..24].copy_from_slice(&sender_mac);
            reply[24..28].copy_from_slice(&sender_ip);
            self.send_frame(sender_mac, ETH_TYPE_ARP, &reply);
            crate::println!(
                "arp: reply {}@{}",
                fmt_ip(&sender_ip),
                fmt_mac(&sender_mac)
            );
        } else if op == 2 && target_ip == OUR_IP {
            crate::println!(
                "arp: got reply from {}@{}",
                fmt_ip(&sender_ip),
                fmt_mac(&sender_mac)
            );
        }
    }

    /// 发送 ARP 请求（who-has target）。
    fn send_arp_request(&mut self, target_ip: [u8; 4]) {
        let broadcast = [0xFF; 6];
        let mut req = [0u8; 28];
        req[0..2].copy_from_slice(&1u16.to_be_bytes());
        req[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
        req[4] = 6;
        req[5] = 4;
        req[6..8].copy_from_slice(&1u16.to_be_bytes()); // op=request
        req[8..14].copy_from_slice(&self.mac);
        req[14..18].copy_from_slice(&OUR_IP);
        req[18..24].copy_from_slice(&[0; 6]);
        req[24..28].copy_from_slice(&target_ip);
        self.send_frame(broadcast, ETH_TYPE_ARP, &req);
    }
}

// ---- 端口 I/O（u8/u16/u32）----

fn io_read8(port: u16) -> u8 {
    let v: u8;
    // SAFETY: 端口为 virtio 配置寄存器。
    unsafe { asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

fn io_write8(port: u16, val: u8) {
    // SAFETY: 端口为 virtio 配置寄存器。
    unsafe { asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)) };
}

fn io_read16(port: u16) -> u16 {
    let v: u16;
    // SAFETY: 端口为 virtio 配置寄存器。
    unsafe { asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

fn io_write16(port: u16, val: u16) {
    // SAFETY: 端口为 virtio 配置寄存器。
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags)) };
}

fn io_read32(port: u16) -> u32 {
    let v: u32;
    // SAFETY: 端口为 virtio 配置寄存器。
    unsafe { asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

fn io_write32(port: u16, val: u32) {
    // SAFETY: 端口为 virtio 配置寄存器。
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags)) };
}

// ---- PCI 配置空间 ----

fn pci_read32(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((reg as u32) & 0xFC);
    io_write32(PCI_CONFIG_ADDR, addr);
    io_read32(PCI_CONFIG_DATA)
}

fn pci_write32(bus: u8, dev: u8, func: u8, reg: u8, val: u32) {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((reg as u32) & 0xFC);
    io_write32(PCI_CONFIG_ADDR, addr);
    io_write32(PCI_CONFIG_DATA, val);
}

/// 扫描总线 0 找 virtio-net。
fn find_virtio_net() -> Option<(u8, u8)> {
    for dev in 0..32u8 {
        for func in 0..8u8 {
            let id = pci_read32(0, dev, func, 0);
            let vendor = (id & 0xFFFF) as u16;
            let device = (id >> 16) as u16;
            if vendor == VIRTIO_PCI_VENDOR && device == VIRTIO_PCI_DEVICE_NET {
                return Some((dev, func));
            }
        }
    }
    None
}

fn fmt_mac(m: &[u8; 6]) -> alloc::string::String {
    alloc::format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5]
    )
}

fn fmt_ip(ip: &[u8; 4]) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

// ---- 全局设备与初始化 ----

static NET: spin::Lazy<spin::Mutex<VirtioNet>> = spin::Lazy::new(|| {
    spin::Mutex::new(unsafe { init_driver() })
});

/// 初始化 virtio-net（PCI 传统模式 + 队列 + 特性协商）。
unsafe fn init_driver() -> VirtioNet {
    let (dev, _func) = find_virtio_net().expect("virtio-net: not found");
    let bar0 = pci_read32(0, dev, 0, 0x10);
    assert!(bar0 & 1 != 0, "virtio-net: BAR0 not IO");
    let io = (bar0 & !0x3) as u16;
    pci_write32(0, dev, 0, 0x04, 0x07); // IO + MEM + bus master

    io_write8(io + REG_STATUS, VIRTIO_ACKNOWLEDGE | VIRTIO_DRIVER);
    let host = io_read32(io + REG_HOST_FEATURES);
    let guest = ((1u32 << VIRTIO_NET_F_MAC)
        | (1u32 << VIRTIO_NET_F_STATUS)
        | (1u32 << VIRTIO_NET_F_NO_CSUM)
        | (1u32 << VIRTIO_NET_F_NO_VLAN))
        & host;
    io_write32(io + REG_GUEST_FEATURES, guest);
    io_write8(
        io + REG_STATUS,
        VIRTIO_ACKNOWLEDGE | VIRTIO_DRIVER | VIRTIO_FEATURES_OK,
    );

    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = io_read8(io + REG_DEVICE_CFG + i as u16);
    }

    let rxq = Virtq::new(0, io);
    let txq = Virtq::new(1, io);
    io_write8(
        io + REG_STATUS,
        VIRTIO_ACKNOWLEDGE | VIRTIO_DRIVER | VIRTIO_FEATURES_OK | VIRTIO_DRIVER_OK,
    );

    let mut net = VirtioNet {
        io,
        mac,
        rxq,
        txq,
        rx_pages: [0; RX_BUFS],
        tx_next: RX_BUFS as u16,
    };
    // 预填 rx 描述符（固定 index 0..RX_BUFS）
    for i in 0..RX_BUFS {
        let page = mm::alloc_pages(1); // 8KB（2KB 数据用）
        assert!(page != 0, "virtio-net: rx buf alloc failed");
        net.rx_pages[i] = page;
        // SAFETY: 清零并写 WRITE 描述符。
        unsafe {
            core::ptr::write_bytes(page as *mut u8, 0, RX_BUF_SIZE);
            let desc = net.rxq.desc.add(i);
            desc.write_volatile(VringDesc {
                addr: page as u64,
                len: RX_BUF_SIZE as u32,
                flags: DESC_F_WRITE,
                next: 0,
            });
            // 直接追加到 avail ring（等价 kick 前批量填充）
            let ai = core::ptr::read_volatile(net.rxq.avail_idx);
            core::ptr::write_volatile(net.rxq.avail_ring.add(ai as usize), i as u16);
            core::ptr::write_volatile(net.rxq.avail_idx, ai.wrapping_add(1));
        }
    }
    io_write16(io + REG_QUEUE_NOTIFY, 0); // kick rx

    crate::println!(
        "virtio-net: io={io:#x} mac={} features={guest:#x}",
        fmt_mac(&mac)
    );
    net
}

/// 启动日志 + 主动 ARP 请求网关（验证收发回路）。
pub fn init() {
    let mac = NET.lock().mac;
    crate::println!("net: virtio-net up, mac={}", fmt_mac(&mac));
    crate::println!("net: arp who-has {}", fmt_ip(&GATEWAY_IP));
    NET.lock().send_arp_request(GATEWAY_IP);
}

/// 轮询接收：处理已完成的 rx 描述符并重新投递。
pub fn net_poll() {
    let mut net = NET.lock();
    let io = net.io;
    loop {
        let item = net.rxq.used_pop();
        match item {
            Some((id, len)) => {
                let idx = id as usize;
                if idx >= RX_BUFS {
                    continue;
                }
                let page = net.rx_pages[idx];
                // SAFETY: 收到的数据（设备在帧前写 10 字节 virtio_net_hdr）。
                let raw = unsafe { core::slice::from_raw_parts(page as *const u8, len as usize) };
                // 跳过 virtio_net_hdr(10)，取以太网帧
                let buf = if raw.len() >= 10 { &raw[10..] } else { raw };
                net.handle_frame(buf);
                // 重新投递该描述符
                // SAFETY: 清零并重置 WRITE 描述符。
                unsafe {
                    core::ptr::write_bytes(page as *mut u8, 0, RX_BUF_SIZE);
                    let desc = net.rxq.desc.add(idx);
                    desc.write_volatile(VringDesc {
                        addr: page as u64,
                        len: RX_BUF_SIZE as u32,
                        flags: DESC_F_WRITE,
                        next: 0,
                    });
                }
                net.rxq.kick(idx as u16, io, 0);
            }
            None => break,
        }
    }
}
