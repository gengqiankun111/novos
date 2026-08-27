//! M2 切片3：虚拟地址空间 + 懒分配 + COW（fork 写时复制）。
//!
//! 每个任务拥有独立 `AddressSpace`（VMA 表：虚拟区间 → 物理页 + COW 标志）。
//! - **懒分配**：`mmap` 只登记虚拟区间（paddr=0），首次 `touch`/写才分配物理页；
//! - **COW**：`fork` 时父子共享物理页（引用计数 +1，双向标记 cow），任一方写入
//!   触发复制新页并解除 COW——物理页互相独立；
//! - 物理页经 mm buddy 分配，引用计数管理（fork 共享 +1，COW 复制后旧页 -1）。
//!
//! 对应 DEVELOPMENT M2：mmap/munmap、懒分配、COW；真正的 4 级页表 + CR3 切换在
//! M3 用户态接入（本切片用软件 VMA 抽象承载 fork 语义验证）。

#![allow(static_mut_refs)]

use crate::mm;
use crate::task;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 页大小。
pub const PAGE_SIZE: usize = 4096;
/// 每地址空间 VMA 上限。
pub const MAX_VMAS: usize = 32;
/// 地址空间表上限（与任务表一致）。
pub const MAX_AS: usize = 16;

/// 虚拟内存区域。
#[derive(Clone, Copy)]
pub struct Vma {
    pub vaddr: usize,
    /// 物理页地址；0 = 尚未分配（懒分配）。
    pub paddr: usize,
    /// COW：写前须复制物理页。
    pub cow: bool,
    pub len: usize,
}

/// 每任务地址空间（VMA 表，线性查找）。
#[derive(Clone, Copy)]
pub struct AddressSpace {
    vmas: [Vma; MAX_VMAS],
    count: usize,
}

impl AddressSpace {
    const fn new() -> Self {
        AddressSpace {
            vmas: [Vma { vaddr: 0, paddr: 0, cow: false, len: 0 }; MAX_VMAS],
            count: 0,
        }
    }
}

/// 每任务地址空间表（下标 = 任务 id）。
static mut AS: [AddressSpace; MAX_AS] = [AddressSpace::new(); MAX_AS];
/// 虚拟地址分配游标（演示用，从 4MB 起线性增长）。
static NEXT_VA: AtomicUsize = AtomicUsize::new(0x4000_0000);
/// 物理页引用计数（下标 = (addr - MEM_START) / PAGE_SIZE）。
static mut PAGE_REFS: [u32; mm::MEM_PAGES] = [0; mm::MEM_PAGES];

fn ref_index(addr: usize) -> usize {
    (addr - mm::MEM_START) / PAGE_SIZE
}

/// 分配一物理页（buddy order 0），引用计数置 1。
pub fn alloc_phys_page() -> usize {
    let p = mm::alloc_pages(0);
    assert!(p != 0, "vmm: phys page OOM");
    // SAFETY: p 在受管区。
    unsafe { PAGE_REFS[ref_index(p)] = 1 };
    p
}

/// 物理页引用计数 +1（fork 共享）。
pub fn add_ref(addr: usize) {
    // SAFETY: addr 为已分配物理页。
    unsafe { PAGE_REFS[ref_index(addr)] += 1 };
}

/// 物理页引用计数 -1；归零归还 buddy。
pub fn release(addr: usize) {
    // SAFETY: addr 为已分配物理页。
    unsafe {
        let i = ref_index(addr);
        if PAGE_REFS[i] > 1 {
            PAGE_REFS[i] -= 1;
        } else if PAGE_REFS[i] == 1 {
            PAGE_REFS[i] = 0;
            mm::free_pages(addr, 0);
        }
    }
}

/// 线性查找 VMA 下标。
fn find_vma(as_: &AddressSpace, vaddr: usize) -> Option<usize> {
    for i in 0..as_.count {
        if as_.vmas[i].vaddr == vaddr {
            return Some(i);
        }
    }
    None
}

/// 为任务 `id` 的地址空间分配虚拟区间（懒分配，不分配物理页）。
pub fn mmap(id: usize, len: usize) -> usize {
    let vaddr = NEXT_VA.fetch_add(len, Ordering::Relaxed);
    // SAFETY: 单核启动/任务上下文。
    unsafe {
        let as_ = &mut AS[id];
        assert!(as_.count < MAX_VMAS, "vmm: vma table full");
        as_.vmas[as_.count] = Vma {
            vaddr,
            paddr: 0,
            cow: false,
            len,
        };
        as_.count += 1;
    }
    vaddr
}

/// munmap：释放 VMA 及其物理页（若有）。
pub fn munmap(id: usize, vaddr: usize) -> bool {
    // SAFETY: 单核。
    unsafe {
        let as_ = &mut AS[id];
        if let Some(i) = find_vma(as_, vaddr) {
            if as_.vmas[i].paddr != 0 {
                release(as_.vmas[i].paddr);
            }
            as_.vmas.copy_within(i + 1..as_.count, i);
            as_.count -= 1;
            true
        } else {
            false
        }
    }
}

/// 懒分配：确保 VMA 已分配物理页（首次访问触发）。
pub fn touch(id: usize, vaddr: usize) {
    // SAFETY: 单核。
    unsafe {
        let as_ = &mut AS[id];
        if let Some(i) = find_vma(as_, vaddr) {
            if as_.vmas[i].paddr == 0 {
                as_.vmas[i].paddr = alloc_phys_page();
            }
        }
    }
}

/// VMA 的物理页地址（调试/验证用）。
pub fn phys(id: usize, vaddr: usize) -> usize {
    // SAFETY: 单核读。
    unsafe {
        let as_ = &AS[id];
        find_vma(as_, vaddr).map(|i| as_.vmas[i].paddr).unwrap_or(0)
    }
}

/// 读 u32（经物理页，恒等映射可直接访问）。
pub fn read_u32(id: usize, vaddr: usize) -> u32 {
    let p = phys(id, vaddr);
    assert!(p != 0, "vmm: read unmapped page");
    // SAFETY: p 为已分配物理页。
    unsafe { core::ptr::read_volatile(p as *const u32) }
}

/// 写 u32：懒分配 + COW 检查（cow 时复制物理页后写）。
pub fn write_u32(id: usize, vaddr: usize, val: u32) {
    // SAFETY: 单核。
    unsafe {
        let as_ = &mut AS[id];
        let i = find_vma(as_, vaddr).expect("vmm: write unmapped vaddr");
        let vma = &mut as_.vmas[i];
        if vma.paddr == 0 {
            vma.paddr = alloc_phys_page();
        }
        if vma.cow {
            let new = alloc_phys_page();
            // 复制旧页内容到新页
            core::ptr::copy_nonoverlapping(vma.paddr as *const u8, new as *mut u8, PAGE_SIZE);
            release(vma.paddr);
            vma.paddr = new;
            vma.cow = false;
        }
        // SAFETY: paddr 为已分配物理页。
        core::ptr::write_volatile(vma.paddr as *mut u32, val);
    }
}

/// fork：把父任务（current_id）的地址空间克隆给子任务 `child_id`，
/// 物理页共享 + 双向 COW 标记。
pub fn on_fork(child_id: usize) {
    let pid = task::current_id();
    // SAFETY: 单核。
    unsafe {
        AS[child_id] = AS[pid]; // VMA 表浅拷贝（物理页地址共享）
        for i in 0..AS[child_id].count {
            let v = &mut AS[child_id].vmas[i];
            if v.paddr != 0 {
                add_ref(v.paddr);
                v.cow = true;
            }
        }
        for i in 0..AS[pid].count {
            if AS[pid].vmas[i].paddr != 0 {
                AS[pid].vmas[i].cow = true;
            }
        }
    }
}

/// 任务退出：释放其地址空间全部物理页引用。
pub fn release_as(id: usize) {
    // SAFETY: 单核。
    unsafe {
        let as_ = &mut AS[id];
        for i in 0..as_.count {
            if as_.vmas[i].paddr != 0 {
                release(as_.vmas[i].paddr);
            }
        }
        as_.count = 0;
    }
}

// ---- M13-01：/proc/self/maps 映射注册表 ----

/// 地址空间映射条目（maps 一行：范围/权限/偏移/路径）。
pub struct MapEntry {
    pub start: u64,
    pub end: u64,
    /// 4 字节权限位："r-xp" / "rw-p" 等。
    pub perms: [u8; 4],
    pub offset: u64,
    /// NUL 结尾路径（"/init" 或 "[stack]"）。
    pub path: [u8; 32],
}

/// 当前进程地址空间映射（`elf::load_and_run` 注册；`/proc/self/maps` 读取）。
pub static MAPS: spin::Lazy<spin::Mutex<alloc::vec::Vec<MapEntry>>> =
    spin::Lazy::new(|| spin::Mutex::new(alloc::vec::Vec::new()));

/// 注册一段映射（ELF PT_LOAD 段 / 用户栈）。
pub fn register_map(start: u64, end: u64, perms: [u8; 4], offset: u64, path: &str) {
    let mut p = [0u8; 32];
    let n = core::cmp::min(path.len(), 31);
    p[..n].copy_from_slice(&path.as_bytes()[..n]);
    MAPS.lock().push(MapEntry {
        start,
        end,
        perms,
        offset,
        path: p,
    });
}

/// 生成 `/proc/self/maps` 内容（Linux 风格：`范围 权限 偏移 设备 inode 路径`）。
pub fn maps_content() -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for e in MAPS.lock().iter() {
        let perms = core::str::from_utf8(&e.perms).unwrap_or("????");
        let path = match e.path.iter().position(|&b| b == 0) {
            Some(n) => core::str::from_utf8(&e.path[..n]).unwrap_or(""),
            None => "",
        };
        s.push_str(&alloc::format!(
            "{:016x}-{:016x} {} {:08x} 00:00 0 {}\n",
            e.start, e.end, perms, e.offset, path
        ));
    }
    s
}
