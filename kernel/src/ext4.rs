//! M10-切片2：ext4-lite 持久化文件系统 + Page Cache 写回。
//!
//! ext4 风格布局的务实简化（DEMO 规模，文档见 DEVELOPMENT M10）：
//! - sector 0：superblock（magic "NXFS" + file_count + next_block）；
//! - sector 16..24：文件表（8 槽 × 512B：name[32] + size + start_block + used）；
//! - sector 24+：数据块（1KB = 2 扇区，块 i 占扇区 24+2i, 24+2i+1）；
//! - Page Cache：块级缓存（读命中 / 写标脏），`blkfs_sync` 落盘，
//!   `blkfs_drop` 清缓存模拟重启——重启后仍能读回（持久化验证）。
//!
//! 说明：sector 2 / 9 保留给 blktest（扇区往返验证），不与之冲突。

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::block::{bio_read, bio_write, SECTOR_SIZE};

const FS_MAGIC: u32 = 0x4E58_4653; // "NXFS"
const MAX_FILES: usize = 8;
const TABLE_START: u64 = 16; // 文件表起始扇区
const DATA_START_SECTOR: u64 = 24; // 数据区起始扇区（1KB 块 = 2 扇区）
const BLOCK_SIZE: usize = 1024;
pub const NAME_MAX: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
struct Superblock {
    magic: u32,
    file_count: u32,
    next_block: u32,
    _pad: [u8; 500],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FileSlot {
    name: [u8; NAME_MAX],
    size: u32,
    start_block: u32,
    used: u32,
    _pad: [u8; 468],
}

impl FileSlot {
    fn empty() -> FileSlot {
        FileSlot {
            name: [0; NAME_MAX],
            size: 0,
            start_block: 0,
            used: 0,
            _pad: [0; 468],
        }
    }
}

/// Page Cache：块级缓存（index = 数据块号，dirty = 待写回）。
struct CachedBlock {
    index: u32,
    dirty: bool,
    data: [u8; BLOCK_SIZE],
}

static CACHE: spin::Lazy<Mutex<Vec<CachedBlock>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

// ---- 扇区级读写 ----

fn read_super() -> Superblock {
    let mut buf = [0u8; SECTOR_SIZE];
    let _ = bio_read(0, &mut buf);
    // SAFETY: buf 512B，Superblock repr(C) 同尺寸。
    unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Superblock) }
}

fn write_super(sb: &Superblock) {
    let mut buf = [0u8; SECTOR_SIZE];
    // SAFETY: 同布局拷贝。
    unsafe { core::ptr::write_unaligned(buf.as_mut_ptr() as *mut Superblock, *sb) };
    let _ = bio_write(0, &buf);
}

fn read_slot(idx: usize) -> FileSlot {
    let mut buf = [0u8; SECTOR_SIZE];
    let _ = bio_read(TABLE_START + idx as u64, &mut buf);
    // SAFETY: buf 512B，FileSlot repr(C) 同尺寸。
    unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const FileSlot) }
}

fn write_slot(idx: usize, s: &FileSlot) {
    let mut buf = [0u8; SECTOR_SIZE];
    // SAFETY: 同布局拷贝。
    unsafe { core::ptr::write_unaligned(buf.as_mut_ptr() as *mut FileSlot, *s) };
    let _ = bio_write(TABLE_START + idx as u64, &buf);
}

/// 按名字找文件槽位。
fn find_slot(name: &str) -> Option<usize> {
    for i in 0..MAX_FILES {
        let s = read_slot(i);
        if s.used != 0 {
            let nlen = s.name.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
            if &s.name[..nlen] == name.as_bytes() {
                return Some(i);
            }
        }
    }
    None
}

// ---- Page Cache ----

/// 读数据块：缓存命中直接返回；否则从盘读入并缓存。
fn cache_read_block(idx: u32) -> Option<[u8; BLOCK_SIZE]> {
    {
        let c = CACHE.lock();
        if let Some(b) = c.iter().find(|b| b.index == idx) {
            return Some(b.data);
        }
    }
    let mut data = [0u8; BLOCK_SIZE];
    let s = DATA_START_SECTOR + idx as u64 * 2;
    if bio_read(s, &mut data[..SECTOR_SIZE]) != 0 {
        return None;
    }
    if bio_read(s + 1, &mut data[SECTOR_SIZE..]) != 0 {
        return None;
    }
    CACHE.lock().push(CachedBlock {
        index: idx,
        dirty: false,
        data,
    });
    Some(data)
}

/// 写数据块到缓存（缺失先读盘），标脏待 sync。
fn cache_write_block(idx: u32, offset: usize, data: &[u8]) -> bool {
    let in_cache = CACHE.lock().iter().any(|b| b.index == idx);
    if !in_cache && cache_read_block(idx).is_none() {
        return false;
    }
    let mut c = CACHE.lock();
    match c.iter_mut().find(|b| b.index == idx) {
        Some(b) => {
            let n = core::cmp::min(data.len(), BLOCK_SIZE.saturating_sub(offset));
            b.data[offset..offset + n].copy_from_slice(&data[..n]);
            b.dirty = true;
            true
        }
        None => false,
    }
}

// ---- 文件系统操作 ----

/// 格式化（新盘）：写超块 + 清文件表 + 清首数据块。
fn format() {
    let sb = Superblock {
        magic: FS_MAGIC,
        file_count: 0,
        next_block: 0,
        _pad: [0; 500],
    };
    write_super(&sb);
    let empty = [0u8; SECTOR_SIZE];
    for i in 0..MAX_FILES {
        let _ = bio_write(TABLE_START + i as u64, &empty);
    }
    let _ = bio_write(DATA_START_SECTOR, &empty);
    let _ = bio_write(DATA_START_SECTOR + 1, &empty);
}

/// 启动探测：magic 不符则格式化新盘。
pub fn init() {
    let sb = read_super();
    if sb.magic != FS_MAGIC {
        format();
        crate::println!("ext4: formatted (fresh disk)");
    } else {
        crate::println!("ext4: superblock ok ({} files)", sb.file_count);
    }
}

/// 确保已格式化（syscall op 0）。
pub fn blkfs_init() -> i64 {
    if read_super().magic != FS_MAGIC {
        format();
    }
    0
}

/// 创建文件。
pub fn blkfs_create(name: &str) -> i64 {
    if name.is_empty() || name.len() >= NAME_MAX {
        return -22; // EINVAL
    }
    if find_slot(name).is_some() {
        return -17; // EEXIST
    }
    for i in 0..MAX_FILES {
        if read_slot(i).used == 0 {
            let mut ns = FileSlot::empty();
            ns.name[..name.len()].copy_from_slice(name.as_bytes());
            ns.used = 1;
            write_slot(i, &ns);
            let mut sb = read_super();
            sb.file_count += 1;
            write_super(&sb);
            return 0;
        }
    }
    -28 // ENOSPC
}

/// 写文件（offset 起；数据进 Page Cache 标脏，sync 落盘）。
pub fn blkfs_write(name: &str, data: &[u8], offset: usize) -> i64 {
    let idx = match find_slot(name) {
        Some(i) => i,
        None => return -2, // ENOENT
    };
    let mut s = read_slot(idx);
    // 首次写分配数据块（block 0 为合法索引，故以 size==0 判定空文件）
    if s.size == 0 {
        let mut sb = read_super();
        s.start_block = sb.next_block;
        sb.next_block += 1;
        write_super(&sb);
    }
    if !cache_write_block(s.start_block, offset, data) {
        return -5; // EIO
    }
    let new_size = core::cmp::max(s.size, (offset + data.len()) as u32);
    s.size = new_size;
    write_slot(idx, &s);
    0
}

/// 读文件（offset 起；Page Cache 命中/读盘）。
pub fn blkfs_read(name: &str, buf: &mut [u8], offset: usize) -> i64 {
    let idx = match find_slot(name) {
        Some(i) => i,
        None => return -2, // ENOENT
    };
    let s = read_slot(idx);
    if s.size == 0 {
        return 0; // 空文件
    }
    let data = match cache_read_block(s.start_block) {
        Some(d) => d,
        None => return -5, // EIO
    };
    let total = s.size as usize;
    let start = core::cmp::min(offset, total);
    let n = core::cmp::min(buf.len(), total - start);
    buf[..n].copy_from_slice(&data[start..start + n]);
    n as i64
}

/// 删除文件。
pub fn blkfs_unlink(name: &str) -> i64 {
    let idx = match find_slot(name) {
        Some(i) => i,
        None => return -2, // ENOENT
    };
    write_slot(idx, &FileSlot::empty());
    let mut sb = read_super();
    sb.file_count = sb.file_count.saturating_sub(1);
    write_super(&sb);
    0
}

/// 列出文件：写 "name size\n" 到 buf；返回字节数。
pub fn blkfs_list(buf: &mut [u8]) -> i64 {
    let mut out = String::new();
    for i in 0..MAX_FILES {
        let s = read_slot(i);
        if s.used != 0 {
            let nlen = s.name.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
            // SAFETY: 文件名为 ASCII。
            let name = unsafe { core::str::from_utf8_unchecked(&s.name[..nlen]) };
            out.push_str(name);
            out.push(' ');
            out.push_str(&alloc::format!("{}\n", s.size));
        }
    }
    let n = core::cmp::min(out.len(), buf.len());
    buf[..n].copy_from_slice(out.as_bytes());
    n as i64
}

/// 写回所有脏块到盘（fsync 语义）。
pub fn blkfs_sync() -> i64 {
    let dirty: Vec<u32> = CACHE
        .lock()
        .iter()
        .filter(|b| b.dirty)
        .map(|b| b.index)
        .collect();
    for idx in dirty {
        let data = CACHE.lock().iter().find(|b| b.index == idx).unwrap().data;
        let s = DATA_START_SECTOR + idx as u64 * 2;
        if bio_write(s, &data[..SECTOR_SIZE]) != 0 {
            return -5;
        }
        if bio_write(s + 1, &data[SECTOR_SIZE..]) != 0 {
            return -5;
        }
        CACHE.lock().iter_mut().find(|b| b.index == idx).unwrap().dirty = false;
    }
    0
}

/// 清空 Page Cache（模拟重启/内存回收）。
pub fn blkfs_drop() {
    CACHE.lock().clear();
}

/// 供启动日志确认。
pub fn info() -> &'static str {
    "ext4-lite(page-cache) ready"
}
