# Novos‑OS 设计文档

- 版本：v0.1（规划稿）
- 适用目标：x86‑64（后续 aarch64）
- 定位：面向「网关 + 容器」的从零实现内核，Rust 编写，**常驻内存 ≤ 32MB** 为硬性目标。

---

## 1. 总览与设计哲学

Novos‑OS 只解决一个问题：**用最小的内核常驻开销，稳定运行容器工作负载并充当网关。**

三条原则贯穿所有设计：

1. **只保留容器基础设施的完整闭环** —— TCP/IP 完整网络栈、epoll、Namespace、Cgroup、OverlayFS 一个都不能少，其余（图形、大量驱动、兼容层）一律砍掉。
2. **内存按预算工程化** —— 每个子系统有明确的预算上限，超预算视为 bug。缓存类内存（dcache/icache/sk_buff）必须可回收、可 shrink。
3. **性能让位于确定性** —— 在 32MB 内追求可预测的延迟和可控的抖动，而不是极限吞吐。

### 1.1 分层架构

```
┌────────────────────────────────────────────────────────────┐
│  用户态  init │ shell │ 容器运行时(runC-like) │ 网关控制面    │
├────────────────────────────────────────────────────────────┤
│  系统调用层   │ 信号 │ 进程生命周期 │ fd/文件句柄           │
├────────────────────────────────────────────────────────────┤
│  调度器 │ VFS │ 网络栈(TCP/UDP/IP/ARP) │ epoll             │
├────────────────────────────────────────────────────────────┤
│  进程/内存管理 │ Namespace │ Cgroup │ OverlayFS            │
├────────────────────────────────────────────────────────────┤
│  驱动层  virtio‑net / virtio‑blk / 8250 UART / 定时器      │
├────────────────────────────────────────────────────────────┤
│  内核核心  x86‑64 启动 │ 中断/异常 │ 内存(物理+虚拟) │ 时钟  │
└────────────────────────────────────────────────────────────┘
```

### 1.2 内核态 / 用户态边界

- **系统调用**：`syscall` 指令（x86‑64），参数在寄存器，errno 语义与 Linux 对齐（降低 runC / musl 移植成本）。
- **用户态 ABI**：ELF64 + System V psABI，`musl` 静态链接（第一版不做动态加载器）。
- **内核对象句柄**：文件描述符（fd）是唯一 IPC/IO 抽象，容器隔离通过 fd 表 + namespace 组合实现。

---

## 2. 内存目标规划

### 2.1 第一版（无桌面、命令行、具备容器能力）：常驻 ≤ 32MB

```
┌─────────────────────────── 32 MB ───────────────────────────┐
│                                                             │
│  A. 内核代码 + 静态数据（8–12 MB）                            │
│  B. 内核运行开销（12–16 MB）                                  │
│  C. 用户态基础程序（4–6 MB）                                  │
│                                                             │
│  中位预期 ≈ 29 MB，留 3 MB 余量应对碎片与峰值                  │
└─────────────────────────────────────────────────────────────┘
```

#### A. 内核代码 + 静态数据：8–12 MB

| 模块 | 预算 | 说明 |
|---|---|---|
| 内核核心（启动/中断/内存/调度） | 2–3 MB | Rust `opt-level=s` + LTO |
| VFS + 文件系统 | 1.5–2 MB | ramfs/tmpfs/OverlayFS 内核逻辑 |
| 网络栈 | 2–3 MB | TCP/IP 完整栈（代码量最大） |
| Namespace + Cgroup | 0.5–1 MB | 内核管理逻辑，逻辑开销本身很小 |
| 驱动（virtio‑net/blk、UART、timer） | 1–2 MB | 最小驱动集 |
| `.rodata`（静态表/字符串） | 1–2 MB | 挂载表、syscall 表、协议表 |
| `.data/.bss` | 0.5–1 MB | 全局状态、位图 |

#### B. 内核运行开销：12–16 MB

| 组成部分 | 预算 | 可回收？ |
|---|---|---|
| 内核堆（slab/kmem 池） | 4–6 MB | 部分（对象复用） |
| VFS 缓存（dcache + icache） | 2–3 MB | ✅ 可 shrink |
| 网络缓冲（sk_buff 池 + TCB + 路由） | 2–4 MB | ✅ 可回收 |
| 页表（内核直接映射 + 映像） | 1–2 MB | 按需分配 |
| Namespace/Cgroup 管理对象 | 0.5–1 MB | 随对象释放 |
| 调度器/定时器/中断表 | 0.5–1 MB | 固定 |
| OverlayFS 缓存（合并 dentry） | 0.5–1 MB | ✅ 随 dcache 回收 |

#### C. 用户态基础程序：4–6 MB

| 程序 | 预算 |
|---|---|
| init（PID 1，负责容器编排/网关启动） | 0.5–1 MB |
| shell | 1–2 MB |
| musl 运行时（静态链接） | 1–2 MB |
| 每进程内核栈 + 页表 + 地址空间元数据 | 0.5–1 MB |

> 关键口径：**B 中 VFS/网络缓存按“常驻均值”计**，允许瞬时增长但必须提供 shrink 路径；这是 32MB 预算能否兑现的核心工程点。

### 2.2 长期稳定版（生产网关 + 多个 Docker 容器）

- **内核基础开销保持 ≤ 32MB**（与第一版预算一致，容器管理逻辑不放大内核常驻）。
- **容器内存完全独立核算**：Namespace/Cgroup 只是内核管理逻辑；容器应用内存按 `cgroup memory.max` 逐容器隔离，不占用内核预算。
- **网关附加预算（独立跟踪，不计入 32MB 基线）**：

| 组件 | 成本模型 |
|---|---|
| conntrack 表 | 按连接数：约 300 B/条（双向），可老化回收 |
| NAT 规则 / 路由表 | 固定 + 随策略增长，策略级内存 |
| 防火墙规则 | 固定，规则条数线性 |

> 结论：32MB 基线在网关负载下依然成立；容器和 conntrack 的内存是“活内存”，跟随负载动态分配与回收。

### 2.3 明确不做的极端目标

**不追求 8–16MB。** 8–16MB 意味着必须牺牲下述任一固定开销，全部不可接受：

- TCP/IP 完整网络栈（重传、拥塞控制、定时器）→ 内存占大头；
- epoll 就绪队列 + 等待队列；
- cgroup 控制器（memory 统计本身要记账）；
- namespace 对象层级；
- overlayfs 的合并 dentry / copy‑up 状态；
- dcache/icache 最低命中缓存。

过度精简 → 性能断崖、抖动失控，对“网关 + 容器”场景是负优化。

---

## 3. 数据结构设计

> 以下结构体为**第一版规划形态**，用 Rust 表示；`Arc` 承担引用计数（对应 Linux 的 kref），内部可变性用锁原语（§3.9）。

### 3.1 物理内存管理

```rust
/// 每个物理页帧一个描述符（对应 Linux mem_map）
#[repr(C)]
pub struct PageFrame {
    pub flags: u32,           // 状态位：RESERVED | SLAB | BUDDY | LRU | ACTIVE | DIRTY
    pub refcount: u32,        // 引用计数
    pub order: u8,            // 在 buddy 中时的阶数
    pub _pad: [u8; 3],
    pub private: usize,       // slab cache 指针 / free-list 链接
    pub lru: ListEntry,       // LRU 链表节点（可回收页）
}

/// 伙伴系统：free_area[MAX_ORDER+1]，每阶一条空闲链表
pub struct BuddyAllocator {
    pub free_area: [FreeList; MAX_ORDER + 1], // order 0..=10（最小 4K，最大 4MB）
    pub nr_free: [usize; MAX_ORDER + 1],
    pub total_pages: usize,
    pub lock: Spinlock,
}

/// Slab 分配器：按对象大小分 cache
pub struct SlabCache {
    pub size: usize,              // 对象大小
    pub align: usize,
    pub free: Vec<*mut u8>,       // 空闲对象池（优先从 partial slab 取）
    pub full: Vec<*mut Page>,     // 已满 slab 页
    pub partial: Vec<*mut Page>,  // 部分使用 slab 页
    pub lock: Spinlock,
}

pub struct Kmem {
    pub caches: [Option<Box<SlabCache>>; 16], // 固定 size 阶梯：64/128/256/…/4K
    pub pages: BuddyAllocator,
}
```

设计要点：
- **Buddy 只管页帧**；小对象走 Slab，避免内部碎片。
- 对象尺寸阶梯固定（64B 起步、对齐 8B），第一版不做 per‑CPU slab，后续按需加。
- `PageFrame.private` 同时承载 buddy free-list 与 slab 归属，用 `flags` 区分。

### 3.2 虚拟内存

```rust
/// 进程地址空间
pub struct Mm {
    pub pgd: PhysAddr,                       // 顶层页表物理地址
    pub vmas: BTreeMap<VirtAddr, Vma>,       // 红黑树（按起始地址）
    pub mmap_base: VirtAddr,                 // mmap 区域基址
    pub brk: VirtAddr,                       // 堆顶
    pub start_code: VirtAddr,
    pub end_code: VirtAddr,
    pub start_data: VirtAddr,
    pub end_data: VirtAddr,
    pub lock: Spinlock,
}

pub struct Vma {
    pub start: VirtAddr,
    pub end: VirtAddr,
    pub flags: VmFlags,                      // READ|WRITE|EXEC|SHARED|PRIVATE
    pub file: Option<Arc<Inode>>,            // mmap 的文件
    pub offset: usize,                       // 文件内偏移
}

/// 页表条目（x86‑64 4 级）
pub struct Pte(u64); // P | RW | US | PWT | PCD | A | D | PAT | G | …PFN…
```

设计要点：
- 页表**懒分配**（首次访问触发缺页才建中间级），内核直接映射区一次性建好。
- **COW**：`fork` 后父子共享只读页，写时复制；页表项 `P+!RW` + 引用计数。
- 第一版不实现 swap（预算内不放磁盘交换），缺页来源 = 匿名页 + 文件 mmap。

### 3.3 任务 / 进程

```rust
pub enum TaskState { Ready, Running, Sleeping, Zombie, Dead }

pub struct Task {
    pub id: u64,                 // pid（线程组内唯一）
    pub tgid: u64,               // 线程组 id（= 主线程 pid）
    pub state: TaskState,
    pub kernel_stack: VirtAddr,  // 内核栈（8K/16K）
    pub mm: Arc<Mm>,
    pub fs: Arc<FsContext>,      // cwd / root / umask
    pub files: Arc<FileTable>,   // fd 表
    pub ns: Arc<Namespaces>,     // §3.4
    pub cgroup: Arc<Cgroup>,     // §3.5
    pub sched: SchedEntity,      // vruntime 等
    pub signals: SignalState,
    pub parent: Option<Weak<Task>>,
    pub children: Vec<Arc<Task>>,
    pub exit_code: i32,
}

pub struct FsContext {
    pub cwd: Arc<Dentry>,
    pub root: Arc<Dentry>,
    pub umask: u32,
}

pub struct FileTable {
    pub fds: BTreeMap<u32, Arc<File>>, // fd -> File
    pub next_fd: u32,
    pub lock: Spinlock,
}

pub struct File {
    pub dentry: Arc<Dentry>,
    pub f_pos: u64,
    pub f_flags: u32,
    pub op: Arc<FileOps>,        // read/write/ioctl/poll/…
}
```

设计要点：
- `Arc<Mm>/Arc<FsContext>/Arc<FileTable>` → **clone 即 fork 语义**；写时复制由 COW 页表完成。
- 线程 = 共享 `mm/files` 的 Task；进程 = 独占 `mm` 的 Task，用 `tgid` 区分。
- 内核栈固定大小，栈溢出用 guard page 检测（预算内）。

### 3.4 Namespace

```rust
/// 一个进程持有的全部 namespace 引用（对应 Linux nsproxy）
pub struct Namespaces {
    pub mnt:   Arc<MntNamespace>,    // 挂载点视图
    pub pid:   Arc<PidNamespace>,    // pid 编号空间
    pub net:   Arc<NetNamespace>,    // 网络栈视图（设备/路由/socket）
    pub uts:   Arc<UtsNamespace>,    // hostname / domainname
    pub ipc:   Arc<IpcNamespace>,    // System V IPC（第一版可空实现）
    pub user:  Arc<UserNamespace>,   // uid/gid 映射（第一版简化）
    pub cgroup: Arc<CgroupNamespace>,// cgroup 根视图
}

pub struct PidNamespace {
    pub level: u32,
    pub parent: Option<Arc<PidNamespace>>,
    pub pid_map: BTreeMap<u64, Weak<Task>>, // ns 内 pid -> task
    pub last_pid: u64,
}

pub struct MntNamespace {
    pub mounts: Vec<Arc<Mount>>,   // 挂载点树
    pub root: Arc<Dentry>,
}

pub struct NetNamespace {
    pub devices: Vec<Arc<NetDevice>>,
    pub routes: RouteTable,
    pub loopback: Arc<NetDevice>,
    pub ip_forwarding: bool,
    pub nat: NatState,
}
```

设计要点：
- 全部用 `Arc` 共享 + `Weak` 破环，namespace 生命周期 = 引用计数归零。
- **pid namespace**：`pid` 是“ns 内编号”，`tgid` 同理；跨 ns 可见性通过遍历父链。
- 网络栈天然按 ns 隔离：每个 `NetNamespace` 独立设备/路由/NAT 表 —— 这是容器“独立网络”的基础。

### 3.5 Cgroup（v2 语义）

```rust
pub struct Cgroup {
    pub id: u64,
    pub name: String,
    pub parent: Option<Arc<Cgroup>>,
    pub children: BTreeMap<String, Arc<Cgroup>>,
    pub controllers: ControllerSet,   // Memory|Cpu|Pids|Io
    pub memory: MemoryController,
    pub cpu: CpuController,
    pub pids: PidsController,
}

pub struct MemoryController {
    pub max: u64,                 // memory.max（字节）
    pub current: AtomicU64,       // 已用字节（逐页记账）
    pub high: u64,                // memory.high（软上限，触发回收）
    pub stat: MemStat,            // anon/file/kernel/…
    pub events: MemEvents,        // max/high 触发计数
}

pub struct CpuController {
    pub weight: u64,              // cpu.weight（带宽权重）
    pub usage: u64,               // 累计 CPU 时间
    pub max_burst: Duration,
}

pub struct PidsController {
    pub max: u64,
    pub current: AtomicU64,
}

/// cgroup 树（v2 单一层级，挂在统一根上）
pub struct CgroupRoot {
    pub root: Arc<Cgroup>,
    pub subsystems: SubsystemSet,
}
```

设计要点：
- **v2 单一层级**，避免 v1 多层级的内存放大，管理成本更低。
- **memory 记账**在页分配路径上做（`page_charge`/`page_uncharge`），超 `memory.max` 触发回收或 OOM-kill（容器内 kill，不波及宿主）。
- 第一版实现 memory / pids 两个控制器即可跑容器；cpu 用 weight 做带宽隔离。

### 3.6 VFS

```rust
pub struct SuperBlock {
    pub fs_type: Arc<FsType>,     // ramfs/tmpfs/overlayfs/…
    pub root: Arc<Dentry>,
    pub s_magic: u32,
    pub inodes: RwLock<HashMap<u64, Arc<Inode>>>, // ino -> inode
}

pub struct Inode {
    pub ino: u64,
    pub mode: FileMode,           // type + 权限位
    pub size: u64,
    pub blocks: u64,
    pub ops: Arc<InodeOps>,       // lookup/create/unlink/…
    pub state: InodeState,        // CLEAN|DIRTY|LOCKED
    pub sb: Arc<SuperBlock>,
    pub data: Arc<dyn FsData>,    // fs 私有数据（fs 的具体实现）
}

pub struct Dentry {
    pub name: String,
    pub parent: Option<Arc<Dentry>>,
    pub inode: Arc<Inode>,
    pub mount: Option<Arc<Mount>>,  // 挂载点（跨 fs 跳转）
    pub state: DentryState,         // 是否已哈希/是否 negative
}

pub struct Mount {
    pub mnt_parent: Option<Arc<Mount>>,
    pub mnt_root: Arc<Dentry>,
    pub mnt_point: Arc<Dentry>,     // 挂载点
    pub mnt_sb: Arc<SuperBlock>,
    pub flags: u32,
}

/// dcache：hash + LRU 可回收
pub struct DCache {
    pub hash: HashMap<(u64, &'static str), Arc<Dentry>>, // parent_ino+name -> dentry
    pub lru: LruList<Arc<Dentry>>,   // 未引用条目，LRU 淘汰
    pub shrink_target: usize,        // 预算内上限
}
```

设计要点：
- **dentry 是路径解析的缓存单元**，`dcache.hash` 命中可跳过磁盘/下层查找；`lru` 提供 shrink 路径（§5.4 预算兑现点）。
- 挂载通过 `Dentry.mount` 跳转：路径解析到挂载点时换 `SuperBlock`。
- `Arc<Inode>` + `Weak<Dentry>` 关系，防止 inode 因 dentry 环泄漏。

### 3.7 OverlayFS

```rust
pub struct OverlayFs {
    pub lower: Vec<Arc<Mount>>,   // lowerdir（只读，可多个）
    pub upper: Arc<Mount>,        // upperdir（可写）
    pub work: Arc<Mount>,         // workdir（copy‑up 暂存）
}

/// overlay 层的“合并 inode”：对上层表现为一个统一文件视图
pub struct OvlInode {
    pub lower: Option<Arc<Inode>>,   // 下层实体（只读）
    pub upper: Option<Arc<Inode>>,   // 上层实体（可写，可能为空）
    pub redirect: Option<String>,    // redirect_dir 目标
    pub state: OvlState,             // UpToDate | NeedsCopyUp | Copying
}

/// 白名单文件（.wh.*）表示“下层被删除/隐藏”
pub struct Whiteout { /* upperdir 中的 .wh.<name> 占位 */ }
```

设计要点：
- **查找**：从 upper 开始逐层向下，第一个命中即止；遇 `.wh.*` 白名单视为“下层不可见”。
- **copy‑up**：写/改上层未有的文件时，先把下层文件复制到 upper，再修改；读不触发。
- `OvlInode.redirect` 支持目录重定向（`redirect_dir` feature），第一版可禁用以省内存。

### 3.8 网络栈

```rust
pub struct NetStack {
    pub devices: Vec<Arc<NetDevice>>,
    pub routes: RouteTable,          // 最长前缀匹配
    pub arp: ArpTable,               // ip -> mac 缓存
    pub conntrack: Conntrack,        // 连接跟踪（网关用）
    pub tcp: TcpLayer,
    pub udp: UdpLayer,
    pub ip: IpLayer,                 // 转发 + 分片（最小化）
    pub softirq: SoftirqQueue,       // 延迟处理队列
}

pub struct NetDevice {
    pub name: String,
    pub mtu: u32,
    pub mac: [u8; 6],
    pub ip: Option<IpAddr>,
    pub rx_queue: VecDeque<Skb>,
    pub tx_queue: VecDeque<Skb>,
    pub flags: NetDevFlags,          // UP|LOOPBACK|RUNNING
    pub ops: Arc<NetDevOps>,         // virtio 收发包
}

pub struct Skb {
    pub data: Vec<u8>,               // 帧/报文数据
    pub dev: Arc<NetDevice>,
    pub proto: u16,
    pub ip: Option<IpHeader>,
    pub tcp: Option<TcpHeader>,
    pub allocated: usize,            // 记账用
}

/// 传输层 socket
pub struct Socket {
    pub family: u16,                 // AF_INET / AF_INET6 / AF_UNIX
    pub sock_type: u16,              // SOCK_STREAM / SOCK_DGRAM
    pub state: SockState,            // LISTEN|SYN_SENT|ESTABLISHED|…
    pub proto: Arc<dyn Proto>,       // TCP / UDP / Unix
    pub local: SockAddr,
    pub peer: SockAddr,
    pub rx_buf: RingBuf<u8>,         // 接收环形缓冲
    pub tx_buf: RingBuf<u8>,
    pub wait: WaitQueue,             // 可读/可写/异常
    pub poll_mask: PollMask,         // epoll 用
}

/// TCP 控制块
pub struct Tcb {
    pub snd_una: u32,  pub snd_nxt: u32,  pub snd_wnd: u32,
    pub rcv_nxt: u32,  pub rcv_wnd: u32,
    pub iss: u32,      pub irs: u32,
    pub state: TcpState,
    pub cwnd: u32,     pub ssthresh: u32,
    pub rto: Duration, pub srtt: Duration,
    pub retrans_timer: Timer,
    pub keepalive: Timer,
    pub out_of_order: BTreeMap<u32, Skb>, // 乱序段
    pub retrans_queue: VecDeque<Skb>,
}

/// epoll：epoll 实例 = 关注 fd 集合 + 就绪队列
pub struct Epoll {
    pub items: HashMap<u32, EpollItem>,   // fd -> 关注事件 + 回调
    pub ready: VecDeque<EpollEvent>,      // 就绪队列（LT/ET）
    pub wait: WaitQueue,
}

pub struct EpollItem {
    pub fd: u32,
    pub events: u32,          // EPOLLIN|EPOLLOUT|…
    pub trigger: TriggerMode, // LevelTriggered / EdgeTriggered
    pub ready: bool,
}
```

设计要点：
- **socket 与协议分离**：`Socket` 面向 fd 层，`Tcb` 是 TCP 专用状态，UDP 只有 `Socket`。
- **epoll 用就绪队列 + 等待队列**实现 O(1) 就绪获取；`Skb.allocated` 让网络缓冲可记账、可回收（预算控制点）。
- **conntrack** 为网关 NAT 服务，条目带老化定时器（§5.7）。

### 3.9 同步原语

```rust
pub struct Spinlock(AtomicBool);          // 短临界区
pub struct Mutex { /* 可睡眠，等待队列 */ }
pub struct WaitQueue { /* sleep 队列 + wake 广播 */ }
pub struct RwLock { /* 读多写少 */ }
pub struct Arc<T>;  pub struct Weak<T>;   // 引用计数共享所有权
// RCU 简化：延迟释放（第一版可用 defer 队列替代）
```

选择原则：
- 中断上下文只用 `Spinlock`；
- 进程上下文可睡眠用 `Mutex`/`RwLock`；
- 共享生命周期用 `Arc/Weak`，**避免手写 kref**，Rust 所有权保证无泄漏。

---

## 4. 核心算法

### 4.1 物理内存分配与回收

**Buddy 分配（分配页帧）：**
1. 从请求阶 `order` 开始向上找第一个非空 `free_area[i]`；
2. 若 `i > order`，逐级分裂，右半块入低一阶链表，左半块继续；
3. 命中页设为 `BUDDY`，`refcount=1`，返回。

**Buddy 释放（合并）：**
1. 检查伙伴页是否空闲（`buddy_addr ^ (1<<(order)<<PAGE_SHIFT)`）；
2. 空闲则合并升阶，递归直到伙伴被占用或到 `MAX_ORDER`；
3. 挂入对应 `free_area`。

**Slab 分配（小对象）：**
1. 优先 per‑partial 空闲对象；没有则从 buddy 取整页建新 slab；
2. 对象出链、初始化、返回；对象回收到 partial；
3. partial 全空 → 整页归还 buddy（内存可回笼）。

**回收路径（shrink）：**
- `dcache.lru` / `icache` 按 LRU 逐出未引用条目 → 释放 inode/dentry 对象到 slab；
- `sk_buff` 超过水位 → 丢弃/压缩（TCP 已确认的段可释放）；
- 匿名页回收：第一版无 swap，优先级最低（只做 cache 回收）；
- 触发点：`memory.high` 越限、全局 low watermark。

### 4.2 调度（CFS 简化）

- 就绪队列 = 按 `vruntime` 排序的红黑树，取最左节点运行；
- `vruntime += 运行时间 / (权重/系统总权重)`；
- 睡眠进程唤醒时 `vruntime` 被 clamp 到最小值附近（防止饿死/抢占）；
- 周期 tick 触发调度点 + 抢占检查；
- 第一版**单核**，SMP 留到稳定版（避免 per‑CPU 复杂度吃掉内存预算）。

```
tick:
    current.vruntime += delta / weight_normalized
    if current.vruntime > runqueue.left.vruntime:
        reschedule()          # 抢占

pick_next:
    return runqueue.left()    # vruntime 最小者
```

### 4.3 路径解析与挂载遍历

```
path_walk(path):
    for component in split(path):
        if cwd 是挂载点: 切到 mnt_root（换 SuperBlock）
        if dentry 命中 dcache.hash: 继续
        else: 调用 inode.ops.lookup 建立 dentry，插入 dcache
    return final dentry + mount
```

- 绝对路径从 `fs.root` 开始，相对路径从 `fs.cwd` 开始；
- 每过一层挂载点跳转更新当前 `Mount`；
- dcache miss 时才落到具体 fs 的 `lookup`，保证热点路径快。

### 4.4 OverlayFS lookup / copy‑up

**lookup（合并查找）：**
```
find_in_overlay(path, ovl):
    for layer in [upper] + reversed(lower):      # 从可写层向下
        if layer 存在 entry:
            if entry 是 whiteout (.wh.<name>): return NULL  # 下层隐藏
            return entry（记录它来自哪层）
    return NULL
```

**copy‑up（写时提升）：**
```
write(ovl_inode, ...):
    if ovl_inode.upper is None:                  # 需要 copy-up
        复制 lower 数据到 upper（经 workdir 暂存，原子 rename）
        ovl_inode.upper = Some(new_inode)
    write 到 upper
```

- 读路径零拷贝（直接读 lower）；只有写才触发 copy‑up；
- 大文件 copy‑up 按需分块，避免一次性吃满内存。

### 4.5 TCP 状态机与拥塞控制

- 状态机：`LISTEN → SYN_RCVD → ESTABLISHED` / `SYN_SENT → ESTABLISHED`；关闭走 `FIN_WAIT/TIME_WAIT`，`TIME_WAIT` 用定时器回收；
- 重传：`rto`（基于 SRTT 指数加权）+ 超时重发，`retrans_queue` 管理；
- 窗口：滑动窗口 + 累计 ACK；乱序段挂 `out_of_order`，对齐后按序投递；
- 拥塞控制：**Cubic 简化版**（第一版可用 NewReno 起步，代码更小），`cwnd`/`ssthresh` 慢启动 + 拥塞避免；
- 定时器：每 TCP 连接一个重传定时器 + keepalive，用**内核统一定时器堆**管理（最小堆）。

### 4.6 容器创建流程（第一版核心）

```
run_container(image, spec):
    1. 创建 pid 子 namespace（新进程在子 ns 内 pid=1）
    2. 创建 net namespace：新建 loopback + veth/virtio 设备
    3. 创建 mnt namespace：准备 rootfs
    4. 挂载 OverlayFS：lower=镜像层, upper=容器层, work
    5. 创建 cgroup 子目录，写入 memory.max / pids.max / cpu.weight
    6. clone(新 task) 进入以上 namespace，pid=1 作为容器 init
    7. pivot_root(overlay 根) + chdir，挂载 /proc（pid ns 视图）
    8. exec 容器 init 进程
```

### 4.7 网关（NAT / 转发）

- **转发**：`ip_forwarding=true` 时，非本机 IP 的包按路由表从对应设备发出（TTL‑1）；
- **SNAT/MASQUERADE**：
  - 出包建立 conntrack 条目 `{内部五元组 → 外部端口}`，改写源 IP/端口；
  - 回包按 conntrack 反向还原；
  - 条目老化（默认 120s 空闲）自动回收；
- **DNAT/端口映射**：入包按规则改写目的地址到容器 IP；
- **基础防火墙**：按 `{dev, src, dst, sport, dport, action}` 顺序匹配（第一版线性表，规则少）。

---

## 5. 内存优化策略

### 5.1 编译期（内核二进制瘦身）

- `Cargo.toml`：`opt-level = "s"`（或 `z`）、`lto = true`、`codegen-units = 1`、`panic = "abort"`；
- `strip = true`（或 `strip = "symbols"`）；`--release` 构建；
- 只链接用到的驱动/协议 feature；`#[no_mangle]` 控制导出面；
- 静态表（syscall 表、协议表）用 `const` 放 `.rodata`，避免运行时构建。

### 5.2 运行期

| 手段 | 收益 |
|---|---|
| 页表/堆懒分配 | 不碰 = 不占内存 |
| dcache/icache LRU + shrink_target | VFS 缓存封顶 |
| sk_buff 池 + 记账 + 水位 | 网络缓冲封顶 |
| Slab 对象复用 + partial 归还 | 内核堆不膨胀 |
| `memory.high` 软上限主动回收 | 避免 OOM 抖动 |
| 用户态静态链接 + strip | 4–6MB 可控 |

### 5.3 预算监控（工程化）

- 每个子系统暴露 `used/limit` 计数（`/proc` 只读视图）；
- CI 里加**内存回归测试**：启动 N 个容器后断言内核常驻 ≤ 32MB；
- 超预算 = bug，进 issue 必修，不允许靠“再优化编译选项”糊弄。

---

## 6. 与 Linux 的对比

| 维度 | 裁剪后主线 Linux（参考） | Novos‑OS |
|---|---|---|
| 空闲常驻 | 50–80 MB | **≤ 32 MB** |
| 兼容层 | 数十年 ABI/驱动包袱 | 无，只保留所需子集 |
| 驱动数 | 成百上千（模块化） | 最小集（virtio/uart/timer） |
| namespace/cgroup | 完整但历史包袱多 | 全新实现，v2 单层 |
| 网络栈 | 完整 + 大量扩展 | 完整 TCP/IP，按需裁剪 |
| 开发语言 | C + 汇编 | **Rust（内存安全）** |
| 可审计性 | 极难 | 预算明确、可测可回归 |

---

*本文档为规划稿，随实现推进持续修订。每个里程碑落地后回填实测内存数据。*
