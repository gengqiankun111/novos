//! M10-切片1：virtio-blk 驱动 + BIO 层（同步 I/O + 错误重试）。
//!
//! - virtio 传统模式 PCI（device 0x1001），单请求队列（同步，一次一个请求）；
//! - `BlockDevice` trait：读/写扇区（512B）+ 容量——M10 后续 ext4/PageCache 依赖；
//! - BIO 层：`bio_read` 失败返回 EIO(-5)；`bio_write` 失败重试 3 次（间隔 ~10ms）；
//! - 请求页布局：16B 请求头 + 512B 数据 + 1B status（同一页，一次分配）。

use crate::mm;
use crate::net::{
    io_read16, io_read32, io_read8, io_write16, io_write32, io_write8, pci_read32, pci_write32,
};

const VIRTIO_PCI_VENDOR: u16 = 0x1AF4;
const VIRTIO_PCI_DEVICE_BLK: u16 = 0x1001;

// virtio 传统模式配置寄存器（I/O BAR 偏移，与 net 相同）
const REG_HOST_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_NUM: u16 = 0x0C;
const REG_QUEUE_SEL: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
const REG_DEVICE_CFG: u16 = 0x14;

// status 位
const VIRTIO_ACKNOWLEDGE: u8 = 1;
const VIRTIO_DRIVER: u8 = 2;
const VIRTIO_DRIVER_OK: u8 = 4;
const VIRTIO_FEATURES_OK: u8 = 8;

pub const SECTOR_SIZE: usize = 512;

// virtio-blk 请求类型
const VIRTIO_BLK_T_IN: u32 = 0; // 读
const VIRTIO_BLK_T_OUT: u32 = 1; // 写

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct UsedElem {
    id: u32,
    len: u32,
}

/// 块设备抽象（M10：PageCache / ext4 依赖此 trait）。
pub trait BlockDevice {
    /// 容量（扇区数）。
    fn capacity(&self) -> u64;
    /// 读一个扇区到 buf（≤512B）。
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()>;
    /// 写一个扇区（data ≤ 512B）。
    fn write_sector(&mut self, lba: u64, data: &[u8]) -> Result<(), ()>;
}

/// virtio-blk（legacy）：单请求队列 + 每请求 3 描述符链（头/数据/status）。
struct VirtioBlk {
    io: u16,
    desc: *mut VringDesc,
    avail_idx: *mut u16,
    avail_ring: *mut u16,
    used_idx: *mut u16,
    used_ring: *mut UsedElem,
    last_used: u16,
    num: u16,
    base: usize,
    sectors: u64,
    /// 请求页：16B 头 @0 + 512B 数据 @16 + 1B status @528。
    req_page: usize,
}

// SAFETY: 驱动仅单核启动/系统调用路径经互斥锁访问，原始指针无跨线程并发。
unsafe impl Send for VirtioBlk {}

impl VirtioBlk {
    /// 扫描总线 0 找 virtio-blk 并初始化。
    ///
    /// # Safety
    /// 仅启动阶段调用一次（单核）。
    unsafe fn new() -> Option<VirtioBlk> {
        let mut found: Option<(u8, u8)> = None;
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(0, dev, func, 0);
                let vendor = (id & 0xFFFF) as u16;
                let device = (id >> 16) as u16;
                if vendor == VIRTIO_PCI_VENDOR && device == VIRTIO_PCI_DEVICE_BLK {
                    found = Some((dev, func));
                }
            }
        }
        let (dev, _func) = found?;
        let bar0 = pci_read32(0, dev, 0, 0x10);
        if bar0 & 1 == 0 {
            return None; // 非 IO BAR
        }
        let io = (bar0 & !0x3) as u16;
        pci_write32(0, dev, 0, 0x04, 0x07); // IO + MEM + bus master

        io_write8(io + REG_STATUS, VIRTIO_ACKNOWLEDGE | VIRTIO_DRIVER);
        // 特性协商：仅需基础（无 VIRTIO_BLK_F_* 必需特性）
        let host = io_read32(io + REG_HOST_FEATURES);
        let _ = host;
        io_write32(io + REG_GUEST_FEATURES, 0);
        io_write8(
            io + REG_STATUS,
            VIRTIO_ACKNOWLEDGE | VIRTIO_DRIVER | VIRTIO_FEATURES_OK,
        );

        // 设备配置：capacity（u64 LE，扇区数）
        let mut cap = [0u8; 8];
        for i in 0..8 {
            cap[i] = io_read8(io + REG_DEVICE_CFG + i as u16);
        }
        let sectors = u64::from_le_bytes(cap);

        // 请求队列（queue 0）
        io_write16(io + REG_QUEUE_SEL, 0);
        let num = io_read16(io + REG_QUEUE_NUM);
        if num < 4 {
            return None;
        }
        let n = num as usize;
        let desc_size = n * 16;
        let avail_off = (desc_size + 1) & !1usize;
        let avail_size = 4 + n * 2;
        let used_off = (avail_off + avail_size + 4095) & !4095usize;
        let used_size = 4 + n * 8;
        let total = used_off + used_size;
        let order = if total > 16 * 1024 { 3 } else { 2 };
        let base = mm::alloc_pages(order);
        if base == 0 {
            return None;
        }
        // SAFETY: 分配的物理页清零。
        unsafe { core::ptr::write_bytes(base as *mut u8, 0, 1usize << (12 + order)) };
        io_write32(io + REG_QUEUE_PFN, (base >> 12) as u32);
        io_write8(
            io + REG_STATUS,
            VIRTIO_ACKNOWLEDGE | VIRTIO_DRIVER | VIRTIO_FEATURES_OK | VIRTIO_DRIVER_OK,
        );

        // 请求页（一次分配：头 + 数据 + status）
        let req_page = mm::alloc_pages(0);
        if req_page == 0 {
            return None;
        }

        Some(VirtioBlk {
            io,
            desc: base as *mut VringDesc,
            avail_idx: (base + avail_off + 2) as *mut u16,
            avail_ring: (base + avail_off + 4) as *mut u16,
            used_idx: (base + used_off + 2) as *mut u16,
            used_ring: (base + used_off + 4) as *mut UsedElem,
            last_used: 0,
            num,
            base,
            sectors,
            req_page,
        })
    }

    /// 同步提交 3 描述符请求并等待完成（带超时）。
    ///
    /// # Safety
    /// 持有 BLK 锁调用（单请求互斥）。
    unsafe fn submit(&mut self, ty: u32, lba: u64, write: bool) -> Result<(), ()> {
        let hp = self.req_page;
        // 请求头：{ type:u32, reserved:u32, sector:u64 }
        core::ptr::write_bytes(hp as *mut u8, 0, 16);
        *(hp as *mut u32) = ty;
        *(hp as *mut u64).add(1) = lba;
        let status_p = hp + 16 + SECTOR_SIZE;
        *(status_p as *mut u8) = 0xFF;

        self.desc.write_volatile(VringDesc {
            addr: hp as u64,
            len: 16,
            flags: DESC_F_NEXT,
            next: 1,
        });
        self.desc.add(1).write_volatile(VringDesc {
            addr: (hp + 16) as u64,
            len: SECTOR_SIZE as u32,
            flags: if write { DESC_F_NEXT } else { DESC_F_NEXT | DESC_F_WRITE },
            next: 2,
        });
        self.desc.add(2).write_volatile(VringDesc {
            addr: status_p as u64,
            len: 1,
            flags: DESC_F_WRITE,
            next: 0,
        });

        // 入 avail ring 并 kick
        let ai = core::ptr::read_volatile(self.avail_idx);
        core::ptr::write_volatile(self.avail_ring.add((ai % self.num) as usize), 0u16);
        core::ptr::write_volatile(self.avail_idx, ai.wrapping_add(1));
        io_write16(self.io + REG_QUEUE_NOTIFY, 0);

        // 等 used（同步；超时保护防挂死）
        let mut spins = 0u32;
        loop {
            let used = core::ptr::read_volatile(self.used_idx);
            if self.last_used != used {
                let idx = self.last_used % self.num;
                let _elem = core::ptr::read_volatile(self.used_ring.add(idx as usize));
                self.last_used = self.last_used.wrapping_add(1);
                break;
            }
            spins += 1;
            if spins > 50_000_000 {
                return Err(());
            }
        }
        let status = *(status_p as *const u8);
        if status != 0 {
            return Err(());
        }
        Ok(())
    }
}

impl BlockDevice for VirtioBlk {
    fn capacity(&self) -> u64 {
        self.sectors
    }

    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        // SAFETY: 先让设备写数据页，再拷回调用方缓冲。
        unsafe { self.submit(VIRTIO_BLK_T_IN, lba, false)? };
        let n = core::cmp::min(buf.len(), SECTOR_SIZE);
        // SAFETY: buf 由调用方保证 ≥ n。
        unsafe {
            core::ptr::copy_nonoverlapping((self.req_page + 16) as *const u8, buf.as_mut_ptr(), n);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
        let n = core::cmp::min(data.len(), SECTOR_SIZE);
        // SAFETY: 先拷数据到请求页，再提交（设备读取）。
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), (self.req_page + 16) as *mut u8, n);
        }
        // SAFETY: 持锁同步提交。
        unsafe { self.submit(VIRTIO_BLK_T_OUT, lba, true)? };
        Ok(())
    }
}

/// 全局块设备（单设备；未探测到则为 None）。
static BLK: spin::Lazy<spin::Mutex<Option<VirtioBlk>>> =
    spin::Lazy::new(|| spin::Mutex::new(unsafe { VirtioBlk::new() }));

/// 启动探测并打印容量。
pub fn init() {
    let mut d = BLK.lock();
    match d.as_ref() {
        Some(b) => {
            let mb = b.capacity() * SECTOR_SIZE as u64 / (1024 * 1024);
            crate::println!("block: virtio-blk up, capacity {} sectors (~{} MiB)", b.capacity(), mb);
        }
        None => crate::println!("block: no virtio-blk device"),
    }
}

/// BIO 读扇区：失败返回 -EIO。
pub fn bio_read(lba: u64, buf: &mut [u8]) -> i64 {
    let mut d = BLK.lock();
    match d.as_mut() {
        Some(b) => match b.read_sector(lba, buf) {
            Ok(()) => 0,
            Err(()) => -5, // EIO
        },
        None => -5, // ENODEV → EIO
    }
}

/// BIO 写扇区：失败重试 3 次（间隔 ~10ms），仍失败返回 -EIO。
pub fn bio_write(lba: u64, data: &[u8]) -> i64 {
    let mut d = BLK.lock();
    match d.as_mut() {
        Some(b) => {
            let mut last = Err(());
            for _ in 0..3 {
                last = b.write_sector(lba, data);
                if last.is_ok() {
                    return 0;
                }
                // 间隔 ~10ms（PIT 100Hz → 1 tick）
                let t0 = crate::task::ticks();
                while crate::task::ticks().wrapping_sub(t0) < 1 {
                    core::hint::spin_loop();
                }
            }
            -5 // EIO
        }
        None => -5,
    }
}

/// 块设备容量（扇区数；无设备返回 0）。
pub fn blk_capacity() -> u64 {
    BLK.lock().as_ref().map(|b| b.capacity()).unwrap_or(0)
}

/// 供启动日志确认。
pub fn info() -> &'static str {
    "block(virtio-blk+bio) ready"
}
