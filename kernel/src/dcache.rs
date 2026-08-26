//! M4-切片3：dcache——路径解析缓存（FNV-1a 哈希桶 + LRU 可回收 + shrink 阈值）。
//!
//! 设计（DESIGN §3.6 / 数据结构评审定案）：
//! - 哈希键用 **FNV-1a**（热路径哈希表禁 SipHash）；
//! - LRU 双向链以 index 实现（intrusive 风格，无堆外开销）；
//! - 键 = (父 inode, 组件名)，值 = 子 inode（Arc 强引用）；
//! - shrink 阈值：`entries > SHRINK_TARGET` 时回收至 `TARGET*0.8`；
//!   `entries > SHRINK_WATERMARK` 时强制立即回收。

use crate::fs::Inode;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// 哈希桶数（2 的幂）。
const BUCKETS: usize = 256;
/// shrink 目标：entries 超过时回收至 80%。
pub const SHRINK_TARGET: usize = 512;
/// 强制水位：超过立即回收（防失控增长）。
pub const SHRINK_WATERMARK: usize = 1024;

const NONE: i32 = -1;

/// FNV-1a 64 位哈希（逐字节喂入 parent 指针 + 名字）。
fn fnv1a(parent: usize, name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for i in 0..8 {
        h ^= ((parent >> (i * 8)) & 0xff) as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &b in name.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct Entry {
    parent: usize,
    inode: Option<Arc<Inode>>,
    name: String,
    prev: i32, // LRU 前驱
    next: i32, // LRU 后继
    hnext: i32, // 同桶链
    active: bool,
}

impl Entry {
    const fn empty() -> Entry {
        Entry {
            parent: 0,
            inode: None,
            name: String::new(),
            prev: NONE,
            next: NONE,
            hnext: NONE,
            active: false,
        }
    }
}

/// dcache：固定哈希桶 + 动态 entry 池。
pub struct DentryCache {
    buckets: [i32; BUCKETS],
    entries: Vec<Entry>,
    head: i32, // LRU 最近
    tail: i32, // LRU 最久
    free: i32, // 空闲 entry 链
    count: usize,
}

impl DentryCache {
    fn new() -> Self {
        DentryCache {
            buckets: [NONE; BUCKETS],
            entries: Vec::new(),
            head: NONE,
            tail: NONE,
            free: NONE,
            count: 0,
        }
    }

    /// 查找：命中则 LRU 提升并返回 inode。
    fn lookup(&mut self, parent: usize, name: &str) -> Option<Arc<Inode>> {
        let b = (fnv1a(parent, name) as usize) % BUCKETS;
        let mut e = self.buckets[b];
        while e != NONE {
            let idx = e as usize;
            if self.entries[idx].active
                && self.entries[idx].parent == parent
                && self.entries[idx].name == name
            {
                self.move_to_head(idx);
                return self.entries[idx].inode.clone();
            }
            e = self.entries[idx].hnext;
        }
        None
    }

    /// 插入（已存在则仅 touch）。
    fn insert(&mut self, parent: usize, name: &str, inode: Arc<Inode>) {
        if self.lookup(parent, name).is_some() {
            return;
        }
        let idx = self.alloc_entry();
        let ent = &mut self.entries[idx];
        ent.active = true;
        ent.parent = parent;
        ent.name = String::from(name);
        ent.inode = Some(inode);
        let b = (fnv1a(parent, name) as usize) % BUCKETS;
        ent.hnext = self.buckets[b];
        self.buckets[b] = idx as i32;
        // LRU 头插
        ent.prev = NONE;
        ent.next = self.head;
        if self.head != NONE {
            self.entries[self.head as usize].prev = idx as i32;
        }
        self.head = idx as i32;
        if self.tail == NONE {
            self.tail = idx as i32;
        }
        self.count += 1;
        // shrink 检查
        if self.count > SHRINK_WATERMARK || self.count > SHRINK_TARGET {
            self.shrink(SHRINK_TARGET * 8 / 10);
        }
    }

    /// 删除（unlink/rmdir 后同步失效）。
    fn remove(&mut self, parent: usize, name: &str) {
        let b = (fnv1a(parent, name) as usize) % BUCKETS;
        let mut e = self.buckets[b];
        let mut prev = NONE;
        while e != NONE {
            let idx = e as usize;
            if self.entries[idx].parent == parent && self.entries[idx].name == name {
                if prev == NONE {
                    self.buckets[b] = self.entries[idx].hnext;
                } else {
                    self.entries[prev as usize].hnext = self.entries[idx].hnext;
                }
                self.free_entry(idx);
                return;
            }
            prev = e;
            e = self.entries[idx].hnext;
        }
    }

    /// 从 LRU 尾部回收直到 count ≤ target。
    fn shrink(&mut self, target: usize) {
        let mut removed = 0usize;
        while self.count > target && self.tail != NONE {
            let t = self.tail as usize;
            // 从哈希桶摘除
            let b = (fnv1a(self.entries[t].parent, &self.entries[t].name) as usize) % BUCKETS;
            let mut e = self.buckets[b];
            let mut prev = NONE;
            while e != NONE && e as usize != t {
                prev = e;
                e = self.entries[e as usize].hnext;
            }
            if e != NONE {
                if prev == NONE {
                    self.buckets[b] = self.entries[t].hnext;
                } else {
                    self.entries[prev as usize].hnext = self.entries[t].hnext;
                }
            }
            self.free_entry(t);
            removed += 1;
        }
        if removed > 0 {
            crate::println!(
                "dcache: shrink -{removed} entries={} target={target}",
                self.count
            );
        }
    }

    fn move_to_head(&mut self, idx: usize) {
        if self.head == idx as i32 {
            return;
        }
        let p = self.entries[idx].prev;
        let n = self.entries[idx].next;
        if p != NONE {
            self.entries[p as usize].next = n;
        }
        if n != NONE {
            self.entries[n as usize].prev = p;
        }
        if self.tail == idx as i32 {
            self.tail = p;
        }
        self.entries[idx].prev = NONE;
        self.entries[idx].next = self.head;
        if self.head != NONE {
            self.entries[self.head as usize].prev = idx as i32;
        }
        self.head = idx as i32;
    }

    fn free_entry(&mut self, idx: usize) {
        let p = self.entries[idx].prev;
        let n = self.entries[idx].next;
        if p != NONE {
            self.entries[p as usize].next = n;
        }
        if n != NONE {
            self.entries[n as usize].prev = p;
        }
        if self.head == idx as i32 {
            self.head = n;
        }
        if self.tail == idx as i32 {
            self.tail = p;
        }
        self.entries[idx].active = false;
        self.entries[idx].inode = None;
        self.entries[idx].hnext = self.free;
        self.free = idx as i32;
        self.count -= 1;
    }

    fn alloc_entry(&mut self) -> usize {
        if self.free != NONE {
            let idx = self.free as usize;
            self.free = self.entries[idx].hnext;
            idx
        } else {
            self.entries.push(Entry::empty());
            self.entries.len() - 1
        }
    }

    fn count(&self) -> usize {
        self.count
    }
}

static DCACHE: spin::Lazy<Mutex<DentryCache>> = spin::Lazy::new(|| Mutex::new(DentryCache::new()));

/// 查找 (parent, name) 的缓存 inode。
pub fn dcache_lookup(parent: usize, name: &str) -> Option<Arc<Inode>> {
    DCACHE.lock().lookup(parent, name)
}

/// 插入缓存项（自动触发 shrink）。
pub fn dcache_insert(parent: usize, name: &str, inode: Arc<Inode>) {
    DCACHE.lock().insert(parent, name, inode);
}

/// 删除缓存项（unlink/rmdir 后调用）。
pub fn dcache_remove(parent: usize, name: &str) {
    DCACHE.lock().remove(parent, name);
}

/// 当前缓存条目数。
pub fn dcache_count() -> usize {
    DCACHE.lock().count()
}
