//! M1：物理内存管理 + 内核堆（Buddy + Slab + GlobalAlloc）。
//!
//! 对应 DESIGN.md §3.1，并落实勘误：
//! - 勘误 §9：Slab 空闲对象用**侵入式空闲链表**（不用 `Vec`，防递归分配 OOM）；
//! - 勘误 §10：PageFrame 带 **MIGRATE_MOVABLE** 标记（后续 compact_zone 用）。
//!
//! 受管物理内存：`[MEM_START, MEM_END)`（QEMU -m 64M、内核镜像 1MB 之上 2MB 起）。
//! 大对象（>4K）直接走 Buddy 页；小对象走 Slab 固定 size 阶梯（64B–4K）。

// static mut 引用仅为初始化/链表操作，M1 单核下安全；显式允许该 lint。
#![allow(static_mut_refs)]

use core::alloc::{GlobalAlloc, Layout};

/// 物理页大小。
pub const PAGE_SIZE: usize = 4096;
/// Buddy 最高阶（4KB << 10 = 4MB）。
pub const MAX_ORDER: usize = 10;
/// 受管内存起点（2MB，内核镜像 ~1.1MB 结束，安全在其上）。
pub const MEM_START: usize = 0x0020_0000;
/// 受管内存终点（64MB）。
pub const MEM_END: usize = 0x0400_0000;
/// 受管页数。
pub const MEM_PAGES: usize = (MEM_END - MEM_START) / PAGE_SIZE;

/// Slab size 阶梯：64B..4096B（幂次）。
const SLAB_SIZES: [usize; 7] = [64, 128, 256, 512, 1024, 2048, 4096];
/// 每个 cache 最多 slab 页（16 × 4K = 64KB/类，够 M1 测试）。
const SLAB_MAX_PAGES: usize = 16;

// ---- PageFrame（DESIGN §3.1）----

/// 页帧标志位。
pub const PG_RESERVED: u32 = 1 << 0;
pub const PG_BUDDY: u32 = 1 << 1;
pub const PG_SLAB: u32 = 1 << 2;
pub const PG_MOVABLE: u32 = 1 << 3;

/// 每个物理页一个描述符（DESIGN §3.1，16 字节）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PageFrame {
    pub flags: u32,
    pub refcount: u32,
    pub order: u8,
    pad: [u8; 3],
    /// 侵入式空闲链表下一帧地址（勘误 §9 同款思路：链表头就是对象首字段）。
    pub next: usize,
    /// 预留：LRU 回收链表节点（M9）。
    _lru: usize,
}

impl PageFrame {
    const fn new() -> Self {
        PageFrame {
            flags: 0,
            refcount: 0,
            order: 0,
            pad: [0; 3],
            next: 0,
            _lru: 0,
        }
    }
}

/// PageFrame 数组（.bss 静态，254KB @ 64MB 受管内存）。
static mut PAGE_FRAMES: [PageFrame; MEM_PAGES] = [PageFrame::new(); MEM_PAGES];

// ---- Buddy ----

/// 伙伴系统内部状态（受 `KernelAlloc.inner` 锁保护）。
struct BuddyInner {
    /// 每阶空闲链表头（帧地址；0 = 空）。
    heads: [usize; MAX_ORDER + 1],
}

impl BuddyInner {
    const fn new() -> Self {
        BuddyInner { heads: [0; MAX_ORDER + 1] }
    }

    /// 帧地址 → PAGE_FRAMES 索引。
    fn index_of(&self, addr: usize) -> usize {
        (addr - MEM_START) / PAGE_SIZE
    }

    fn flag(&mut self, addr: usize, bit: u32, set: bool) {
        // SAFETY: addr 在受管范围内，索引有效。
        let f = unsafe { &mut *(&mut PAGE_FRAMES[self.index_of(addr)] as *mut PageFrame) };
        if set {
            f.flags |= bit;
        } else {
            f.flags &= !bit;
        }
    }

    fn set_order(&mut self, addr: usize, order: usize) {
        // SAFETY: 索引有效。
        unsafe {
            PAGE_FRAMES[self.index_of(addr)].order = order as u8;
        }
    }

    fn next_of(&self, addr: usize) -> usize {
        // SAFETY: 只读。
        unsafe { PAGE_FRAMES[self.index_of(addr)].next }
    }

    /// 头插到 order 空闲链表。
    fn push_head(&mut self, addr: usize, order: usize) {
        // SAFETY: 写入 next 字段。
        unsafe {
            PAGE_FRAMES[self.index_of(addr)].next = self.heads[order];
        }
        self.heads[order] = addr;
        self.flag(addr, PG_BUDDY, true);
        self.set_order(addr, order);
    }

    /// 从 order 空闲链表移除（单项链表 O(n) 查找，M1 可接受）。
    fn remove(&mut self, addr: usize, order: usize) {
        let mut prev = 0usize;
        let mut cur = self.heads[order];
        while cur != 0 {
            if cur == addr {
                if prev == 0 {
                    self.heads[order] = self.next_of(cur);
                } else {
                    // SAFETY: prev 在链表中。
                    unsafe {
                        PAGE_FRAMES[self.index_of(prev)].next = self.next_of(cur);
                    }
                }
                self.flag(addr, PG_BUDDY, false);
                return;
            }
            prev = cur;
            cur = self.next_of(cur);
        }
        panic!("mm: buddy remove: addr {:#x} not in order {order} list", addr);
    }

    /// 同阶空闲块地址。
    fn buddy_addr(addr: usize, order: usize) -> usize {
        addr ^ (PAGE_SIZE << order)
    }

    fn is_buddy_free(&self, addr: usize, order: usize) -> bool {
        // 伙伴必须在受管范围内：最低位页（MEM_START）在 order=9 时伙伴为地址 0，
        // index_of(0) 会下溢成非规范地址（#GP）；顶部块同理防越界。
        if addr < MEM_START || addr >= MEM_END {
            return false;
        }
        // SAFETY: 只读。
        let f = unsafe { &*PAGE_FRAMES.get_unchecked(self.index_of(addr)) };
        f.flags & PG_BUDDY != 0 && f.order as usize == order
    }

    /// 分配 order 阶块，返回帧地址；失败返回 0。
    fn alloc(&mut self, order: usize) -> usize {
        for i in order..=MAX_ORDER {
            if self.heads[i] != 0 {
                let addr = self.heads[i];
                self.heads[i] = self.next_of(addr);
                self.flag(addr, PG_BUDDY, false);
                // 分裂：高层块逐级把右半块挂回低一阶
                let mut o = i;
                while o > order {
                    o -= 1;
                    let right = addr + (PAGE_SIZE << o);
                    self.push_head(right, o);
                    self.set_order(addr, o);
                }
                self.set_order(addr, order);
                return addr;
            }
        }
        0 // OOM
    }

    /// 释放 order 阶块，与伙伴逐级合并（合并后以 min 基址挂回更高阶）。
    fn free(&mut self, addr: usize, mut order: usize) {
        let mut base = addr;
        loop {
            let buddy = Self::buddy_addr(base, order);
            if order < MAX_ORDER && self.is_buddy_free(buddy, order) {
                self.remove(buddy, order);
                base = core::cmp::min(base, buddy);
                order += 1;
            } else {
                break;
            }
        }
        self.push_head(base, order);
    }
}

// ---- Slab ----

/// Slab 缓存（每个 size 类一个），空闲对象用侵入式单向链表（勘误 §9）。
struct SlabInner {
    size: usize,
    /// 空闲对象链表头（对象地址）。
    free_list: usize,
    /// 已分配 slab 页（固定数组，防 Vec 扩容递归）。
    pages: [usize; SLAB_MAX_PAGES],
    page_count: usize,
}

impl SlabInner {
    const fn new() -> Self {
        SlabInner {
            size: 0,
            free_list: 0,
            pages: [0; SLAB_MAX_PAGES],
            page_count: 0,
        }
    }

    /// 分配一个对象（buddy 提供新 slab 页）。
    fn alloc(&mut self, buddy: &mut BuddyInner) -> usize {
        if self.free_list != 0 {
            let obj = self.free_list;
            // SAFETY: free_list 指向空闲对象，其首 8 字节存下一指针。
            self.free_list = unsafe { *(obj as *const usize) };
            return obj;
        }
        // 需要新 slab 页
        if self.page_count >= SLAB_MAX_PAGES {
            panic!("mm: slab {} exhausted ({} pages)", self.size, SLAB_MAX_PAGES);
        }
        let page = buddy.alloc(0);
        if page == 0 {
            panic!("mm: slab OOM");
        }
        self.pages[self.page_count] = page;
        self.page_count += 1;
        // 把整页切成 size 对象，串成侵入式链表
        let n = PAGE_SIZE / self.size;
        let mut prev = 0usize;
        for i in (0..n).rev() {
            let addr = page + i * self.size;
            // SAFETY: addr 在刚分配的 slab 页内。
            unsafe { *(addr as *mut usize) = prev };
            prev = addr;
        }
        self.free_list = prev;
        self.alloc(buddy)
    }

    /// 释放对象（头插空闲链表）。
    fn free(&mut self, addr: usize) {
        // SAFETY: 对象在 slab 页内，首 8 字节用作链表指针。
        unsafe { *(addr as *mut usize) = self.free_list };
        self.free_list = addr;
    }
}

// ---- KernelAlloc（Buddy + Slab 统一入口，全局分配器）----

/// 全局内核分配器内部状态。
struct AllocInner {
    buddy: BuddyInner,
    caches: [SlabInner; SLAB_SIZES.len()],
}

impl AllocInner {
    const fn new() -> Self {
        AllocInner {
            buddy: BuddyInner::new(),
            caches: [const { SlabInner::new() }; SLAB_SIZES.len()],
        }
    }

    /// Slab 分配（包装 caches 与 buddy 的分片借用，避免 `self.caches[i].alloc(&mut self.buddy)` 双可变借用）。
    fn slab_alloc(&mut self, idx: usize) -> usize {
        let buddy = &mut self.buddy;
        self.caches[idx].alloc(buddy)
    }

    /// Slab 释放。
    fn slab_free(&mut self, idx: usize, addr: usize) {
        self.caches[idx].free(addr);
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();
        if align > PAGE_SIZE {
            panic!("mm: unsupported alignment {align} > PAGE_SIZE");
        }
        if size <= SLAB_SIZES[SLAB_SIZES.len() - 1] {
            // 找最小可容纳 size 的类；若 align 超过类大小则退回整页分配
            for (i, class) in SLAB_SIZES.iter().enumerate() {
                if size <= *class {
                    if align <= *class {
                        let obj = self.slab_alloc(i);
                        return obj as *mut u8;
                    }
                    break;
                }
            }
        }
        // 大对象：Buddy 整页分配
        let order = ((size + PAGE_SIZE - 1) / PAGE_SIZE).next_power_of_two().trailing_zeros() as usize;
        let order = order.min(MAX_ORDER);
        let page = self.buddy.alloc(order);
        if page == 0 {
            return core::ptr::null_mut();
        }
        page as *mut u8
    }

    fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let addr = ptr as usize;
        let size = layout.size();
        if size <= SLAB_SIZES[SLAB_SIZES.len() - 1] {
            for (i, class) in SLAB_SIZES.iter().enumerate() {
                if size <= *class {
                    // 与 alloc 对齐规则对应：align>class 走 buddy
                    if layout.align() <= *class {
                        self.slab_free(i, addr);
                        return;
                    }
                    break;
                }
            }
        }
        // 大对象回 buddy
        let order = ((size + PAGE_SIZE - 1) / PAGE_SIZE).next_power_of_two().trailing_zeros() as usize;
        self.buddy.free(addr, order.min(MAX_ORDER));
    }
}

/// 全局分配器（单锁覆盖 buddy + slab；M1 单核，M2 起换 per-CPU/细粒度锁）。
pub struct KernelAlloc {
    inner: spin::Mutex<AllocInner>,
}

unsafe impl Sync for KernelAlloc {}

impl KernelAlloc {
    const fn new() -> Self {
        KernelAlloc {
            inner: spin::Mutex::new(AllocInner::new()),
        }
    }
}

/// # Safety
/// 标准 GlobalAlloc 契约。
unsafe impl GlobalAlloc for KernelAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.lock().dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOC: KernelAlloc = KernelAlloc::new();

// ---- 初始化与自测 ----

/// 内存统计（DESIGN §5.3 台账入口）。
pub struct MemStats {
    pub buddy_pages: usize,
    pub slab_pages: usize,
    pub kernel_used_bytes: usize,
}

/// 初始化内存管理：全部受管页并入 buddy（自动合并），可移动页标记。
///
/// # Safety
/// 仅由 rust_start 调用一次，且早于任何分配。
pub unsafe fn init() {
    // 重置所有页帧
    for f in PAGE_FRAMES.iter_mut() {
        *f = PageFrame::new();
    }
    let mut inner = ALLOC.inner.lock();
    // 全部页按 order-0 逐个 free（free 内自动合并成高阶块）
    let mut i = 0usize;
    while i < MEM_PAGES {
        let addr = MEM_START + i * PAGE_SIZE;
        // 页帧全部标记 MOVABLE（勘误 §10：用户态匿名页/PageCache 语义；页表等内核结构后续标 UNMOVABLE）
        inner.buddy.flag(addr, PG_MOVABLE, true);
        inner.buddy.free(addr, 0);
        i += 1;
    }
    // 初始化 slab size 类
    for (cache, class) in inner.caches.iter_mut().zip(SLAB_SIZES.iter()) {
        cache.size = *class;
    }
}

/// 内存统计。
pub fn mem_stats() -> MemStats {
    let inner = ALLOC.inner.lock();
    let mut slab_pages = 0usize;
    for c in inner.caches.iter() {
        slab_pages += c.page_count;
    }
    let free = free_pages_locked(&inner);
    MemStats {
        buddy_pages: MEM_PAGES - free - slab_pages,
        slab_pages,
        kernel_used_bytes: slab_pages * PAGE_SIZE + MEM_PAGES * core::mem::size_of::<PageFrame>(),
    }
}

/// 遍历 buddy 各阶链表统计空闲页（须持锁）。
fn free_pages_locked(inner: &AllocInner) -> usize {
    let mut count = 0usize;
    for (order, head) in inner.buddy.heads.iter().enumerate() {
        let mut cur = *head;
        while cur != 0 {
            count += 1 << order;
            cur = inner.buddy.next_of(cur);
        }
    }
    count
}

/// M1 自测：Buddy / Slab / Vec / Box。
pub fn self_test() -> Result<&'static str, &'static str> {
    // 1. Buddy：alloc/free 往返，分配块不重叠
    {
        let mut inner = ALLOC.inner.lock();
        let a = inner.buddy.alloc(2); // 16K
        let b = inner.buddy.alloc(2);
        let c = inner.buddy.alloc(5); // 128K
        if a == 0 || b == 0 || c == 0 {
            return Err("buddy alloc returned 0 (OOM)");
        }
        // 同阶块必须互不重叠
        let a_end = a + (PAGE_SIZE << 2);
        let b_end = b + (PAGE_SIZE << 2);
        if a_end > b && b_end > a {
            return Err("buddy order-2 blocks overlap");
        }
        inner.buddy.free(c, 5);
        inner.buddy.free(b, 2);
        inner.buddy.free(a, 2);
        // 合并后应能分配出高阶块（order 5 再次成功）
        let d = inner.buddy.alloc(5);
        if d == 0 {
            return Err("buddy merge failed (order-5 alloc after free)");
        }
        inner.buddy.free(d, 5);
    }

    // 2. Slab：对象分配/释放/复用
    {
        let mut inner = ALLOC.inner.lock();
        let o1 = inner.slab_alloc(2); // 256B 类
        let o2 = inner.slab_alloc(2);
        if o1 == 0 || o2 == 0 {
            return Err("slab alloc returned 0");
        }
        inner.slab_free(2, o1);
        // 释放后应能立即复用同一对象
        let o3 = inner.slab_alloc(2);
        if o3 != o1 {
            return Err("slab free-list reuse failed");
        }
        inner.slab_free(2, o2);
        inner.slab_free(2, o3);
    }

    // 3. GlobalAlloc：Vec + Box（经 #[global_allocator]）
    {
        let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(10_000);
        for i in 0..10_000u64 {
            v.push(i);
        }
        let sum: u64 = v.iter().sum();
        if sum != 10_000 * 9_999 / 2 {
            return Err("Vec sum mismatch");
        }
        let b = alloc::boxed::Box::new(42u32);
        if *b != 42 {
            return Err("Box value mismatch");
        }
    }

    Ok("ALL PASS")
}

/// 追踪性分配器统计：空闲页帧数量。
pub fn free_page_count() -> usize {
    let inner = ALLOC.inner.lock();
    free_pages_locked(&inner)
}

/// 从 buddy 分配 `order` 阶物理页块（vmm 用；GlobalAlloc 走 Slab 不直接暴露 buddy）。
pub fn alloc_pages(order: usize) -> usize {
    ALLOC.inner.lock().buddy.alloc(order)
}

/// 释放 buddy 物理页块。
///
/// # Safety
/// addr 必须由 `alloc_pages` 同阶返回。
pub unsafe fn free_pages(addr: usize, order: usize) {
    ALLOC.inner.lock().buddy.free(addr, order);
}

/// 供 main.rs 使用的可分配字节上限常量。
pub const fn heap_capacity_bytes() -> usize {
    MEM_END - MEM_START
}
