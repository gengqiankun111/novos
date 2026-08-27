# 山水观心操作系统远期扩展口（EXTENSIONS）

> **原则**：第一版一律采用最简数据结构（边缘设备规模小、开销可忽略）。
> 本文件登记"评估通过但刻意推迟"的远期优化，全部通过 **trait 接口 + Cargo feature flag**
> 预留扩展口，**不阻塞第一版交付**。触发条件满足前不做实现。
>
> 与 DESIGN.md 的关系：本文件是 DESIGN.md §3.2 / §3.8 / §14"架构级预留"的详细展开。
> 版本节奏见 [VERSIONING.md](VERSIONING.md)（远期项不绑定任何里程碑，按需启用）。

---

## 0. 长期演进方向总览

本文件登记的是**数据结构级**远期项（Maple Tree / rhashtable / trie / 基数树）。
除此之外，山水观心操作系统沿三条主线长期演进（基于 DESIGN §21 的预期管理），有独立文档：

| 文档 | 内容 |
|---|---|
| [DESIGN_EXTENSION.md](DESIGN_EXTENSION.md) | 长期设计：契约式交付（免疫）、透明化体验、深度定制三主线 |
| [DEVELOP_EXTENSION.md](DEVELOP_EXTENSION.md) | 长期开发：三主线任务拆解 + v1.0/v2.0/v3.0 演进节奏 |

三条主线与本文档的关系：本文档的 trait 预留（如 `RouteTable`/`PageCache`）服务于主线三的
性能深水区；主线一/二不涉及本文档的数据结构项。

---

## 1. VMA：Maple Tree（`--features advanced-vma`）

**第一版（已定案）**：`BTreeMap<VirtAddr, Vma>`（红黑树）。
- 容器进程 VMA 数通常 20~100 个，`O(log n) ≈ 7` 次指针跳转，缺页路径开销可忽略；
- mmap/munmap 低频，红黑树维护不是瓶颈；实现约 300 行。

**远期扩展口**：当 **单进程 VMA 数 > 512** 或 **并发缺页成为瓶颈**时，评估迁移
**Maple Tree**（Linux 6.1+ 的 RCU 安全区间 B-Tree）。

**接口预留**（第一版实现为 `BTreeMap` 适配器，切换零侵入）：

```rust
/// VMA 区间树抽象：Maple Tree 与 BTreeMap 皆可实现，feature 切换。
pub trait VmaTree {
    /// 含 addr 的 VMA（区间查找）
    fn find_vma(&self, addr: VirtAddr) -> Option<&Vma>;
    /// 插入区间（重叠检查）
    fn insert(&mut self, vma: Vma) -> Result<(), ()>;
    /// 删除区间
    fn remove(&mut self, start: VirtAddr) -> Option<Vma>;
    /// 前一区间（mmap 合并/遍历用）
    fn find_prev(&self, addr: VirtAddr) -> Option<&Vma>;
}
```

**为什么不是第一版**：
- Maple Tree 约 3000~4000 行，节点（Range Lock + 存储槽）比红黑树节点大 2~3 倍；
- 范围查询/分裂合并逻辑复杂，Rust 下同样易引入内存安全问题；
- 第一版单核（UP）无 RCU 需求。

---

## 2. 可扩展哈希（rhashtable）

**第一版（已定案）**：固定桶数哈希表 + **FNV-1a / xxHash**（热路径**禁用 SipHash**，
见 DESIGN.md §3.6）。哈希碰撞用桶内链表（条目少，链表足够）。

**远期扩展口**：当任一哈希表（conntrack / ARP / dcache）单表条目 > 1K 且 resize 成为瓶颈时，
迁移**可扩展哈希（rhashtable）**：
- **增量 resize**：新旧桶并存、渐进迁移，无"全表拷贝"停顿（内核哈希表核心设计哲学）；
- 桶内不再需要红黑树（链表足够，因为负载因子受控）。

**接口预留**：

```rust
/// 哈希表抽象：固定桶（第一版）与 rhashtable（远期）皆可实现。
pub trait HashTable<K, V> {
    fn insert(&mut self, k: K, v: V) -> Option<V>;
    fn get(&self, k: &K) -> Option<&V>;
    fn remove(&mut self, k: &K) -> Option<V>;
    /// 远期：增量 resize 入口（第一版为 no-op）
    fn resize_if_needed(&mut self) {}
}
```

---

## 3. 路由表：PATRICIA trie

**第一版（已定案）**：线性表（边缘网关路由条目通常数百条以内，线性扫描可接受）。

**远期扩展口**：路由表 > 数百条时，迁移 **PATRICIA trie**（最长前缀匹配，O(长度)）。

**接口预留**：

```rust
pub trait RouteTable {
    fn add(&mut self, prefix: Ipv4Net, nh: Nexthop);
    fn lookup(&self, dst: Ipv4Addr) -> Option<&Nexthop>; // 最长前缀匹配
    fn remove(&mut self, prefix: Ipv4Net) -> bool;
}
```

---

## 4. Page Cache：基数树（radix tree）

**第一版（已定案）**：`HashMap<(ino, offset), 物理页>`。

**远期扩展口**：命中率优化到极致时，按文件组织为**基数树**（`ino → (offset → 页)`），
支持区间遍历与整文件回收。

**接口预留**：

```rust
pub trait PageCache {
    fn get(&mut self, ino: u64, offset: u64) -> Option<PhysAddr>;
    fn insert(&mut self, ino: u64, offset: u64, page: PhysAddr);
    fn evict_range(&mut self, ino: u64, start: u64, end: u64); // shrink
}
```

---

## 启用节奏

| 项 | feature flag | 触发条件 | 状态 |
|---|---|---|---|
| Maple Tree VMA | `advanced-vma` | VMA > 512 或并发缺页瓶颈 | 预留，不实现 |
| rhashtable | `advanced-hashtable` | 单表 > 1K 条目 | 预留，不实现 |
| PATRICIA trie | `advanced-routing` | 路由 > 数百条 | 预留，不实现 |
| 基数树 Page Cache | `advanced-pagecache` | 命中率优化到极致 | 预留，不实现 |

> 所有 trait 的第一版实现均为最简结构适配器；启用远期项 = 换 trait 实现 + feature 开关，
> 不触碰调用方代码。
