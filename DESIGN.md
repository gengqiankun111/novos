# Novos‑OS 设计文档

- 版本：v0.1（规划稿）
- 适用目标：x86‑64 起步（QEMU 开发便利）；**ARM64 为终局目标**（arch 层隔离从第一天做好）
- 定位：面向内存受限设备（256MB–2GB）的**微型容器宿主** —— RTOS 的占地，Linux 的生态，Rust 的安全
- 硬性目标：**常驻内存 ≤ 32MB** 稳定运行容器工作负载

---

## 1. 总览与设计哲学

Novos‑OS 只解决一个问题：**用最小的内核常驻开销，稳定运行容器工作负载并充当网关。**

**定位（2026-08 定稿）**：嵌入式容器宿主 —— 填补 FreeRTOS 与嵌入式 Linux 之间的结构性空白
（RTOS 无 MMU/单进程够不着"多服务隔离 + 容器"；裁剪 Linux 空闲 50–80MB、256MB 设备跑不动）。
兼容策略：Linux syscall ABI + musl 静态子集（自己编译 Go/Rust/C++，见 §15）；≤32MB 是**入场券**，
卖点是**确定性 + 安全 + 可控**（秒级冷启动、低抖动调度、OTA 升级、Rust 内存安全证明）。

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
│  驱动层  bus→device→driver 框架（virtio/uart/timer/GPIO/…） │
├────────────────────────────────────────────────────────────┤
│  内核核心  x86‑64 启动 │ 中断/异常 │ 内存(物理+虚拟) │ 时钟  │
│  arch 层：x86_64（起步）/ aarch64（终局）/ riscv64（留口）  │
└────────────────────────────────────────────────────────────┘
```

> **架构骨架（第一版分层约束，详见 §19）**：① 设备驱动模型（bus→device→driver + BSP）；② RT 调度双队列预留；③ 时钟/中断框架（时钟源抽象 + RTC + monotonic）；④ 快速启动（deferred init）；⑤ arch 层同时留 aarch64 / riscv64 口。

### 1.2 内核态 / 用户态边界

- **系统调用**：`syscall` 指令（x86‑64），参数在寄存器，errno 语义与 Linux 对齐（降低 runC / musl 移植成本）。
- **用户态 ABI**：ELF64 + System V psABI，`musl` 静态链接（第一版不做动态加载器）。
- **内核对象句柄**：文件描述符（fd）是唯一 IPC/IO 抽象，容器隔离通过 fd 表 + namespace 组合实现。

### 1.3 启动流程

```
┌─────────────┐    ┌──────────────┐    ┌─────────────────┐
│ Power-On /  │───▶│  bootloader  │───▶│  kernel_entry    │
│  RESET      │    │ (GRUB/UEFI)   │    │  (_start)        │
└─────────────┘    └──────────────┘    └────────┬────────┘
                                                 │
                 ┌───────────────────────────────▼──────────────┐
                 │              Phase 1: 早期初始化               │
                 │  · 设栈指针 + 栈 guard page                    │
                 │  · 读取 multiboot2 info tag（内存映射/命令行）     │
                 │  · 建立内核直接映射页表（4 级，2MB 大页）          │
                 │  · 加载 GDT + IDT 骨架（仅 panic/异常入口）      │
                 └───────────────────┬──────────────────────────┘
                                     │
                 ┌───────────────────▼──────────────────────────┐
                 │              Phase 2: 子系统初始化              │
                 │  · 物理内存管理（buddy 初始化，标记可用区域）      │
                 │  · 内核堆（slab 分配器，GlobalAlloc 接入）        │
                 │  · 完整 IDT（中断 + 异常 + syscall 入口）        │
                 │  · 定时器（HPET / PIT，tick + 定时器堆）         │
                 │  · 8250 UART 驱动（print!/println! 可用）        │
                 │  · VFS 挂载 ramfs 为根                          │
                 │  · 调度器初始化（idle task + 运行队列）           │
                 └───────────────────┬──────────────────────────┘
                                     │
                 ┌───────────────────▼──────────────────────────┐
                 │              Phase 3: 用户态启动                │
                 │  · 加载 init（ELF，PID 1）到首个用户态地址空间    │
                 │  · 建 init 的 fd 表（stdin/stdout → /dev/uart） │
                 │  · 首次切换到用户态（IRET 指令）                 │
                 │  · init fork + exec shell                     │
                 │  · init 监控容器生命周期、回收僵尸               │
                 └──────────────────────────────────────────────┘
```

启动流程关键约束：
- **Phase 1 无堆**——只有栈和静态页表，不能分配动态内存；
- **Phase 2 串行初始化**——子系统有严格顺序依赖（先内存管理，后能用 `Arc`/`Vec`）；
- **Phase 3 首次进入用户态**——从此 CPU 不再直接执行内核代码，只通过 syscall/中断返回。

### 1.4 中断与异常处理

#### IDT 结构

```rust
/// IDT 条目（x86-64 gate descriptor）
#[repr(C, packed)]
pub struct IdtEntry {
    pub offset_low: u16,       // handler 低 16 位
    pub selector: u16,         // 段选择子（内核代码段）
    pub ist: u8,               // IST 索引（独立栈，用于 NMI/DF/MC）
    pub flags: u8,             // P | DPL | Type
    pub offset_mid: u16,       // handler 中 16 位
    pub offset_high: u32,      // handler 高 32 位
    pub _reserved: u32,
}

/// IST 栈表（IST 1–7，各自独立栈，防止嵌套中断栈溢出）
pub struct IstStacks {
    pub df: [u8; 8192],    // IST 1: Double Fault（#DF）
    pub nmi: [u8; 8192],   // IST 2: NMI
    pub mc: [u8; 8192],    // IST 3: Machine Check（#MC）
    pub dbg: [u8; 8192],   // IST 4: Debug（#DB）
}
```

#### 中断向量分配

| 向量 | 类型 | 用途 |
|---|---|---|
| 0–31 | CPU 异常 | #DE/#DB/#PF/#GP/#DF/… 各自独立 handler |
| 32–47 | PIC/IOAPIC 中断 | timer(32)、UART(36)、virtio-net(40)、virtio-blk(44) |
| 128 (0x80) | 系统调用 | `syscall` 指令入口（用户态→内核态） |

#### 异常处理流程

```
exception_handler(vec, frame):
    1. 从 IST 或内核栈取异常帧（寄存器快照）
    2. 判断类型：
       - #PF (page fault): 解析 CR2 → 缺页地址
         · 地址在 VMA 范围内 → 懒分配/COW → 建页表项 → 返回
         · 地址非法 → SIGSEGV 终止进程（内核态 #PF → panic）
       - #GP: 打印寄存器快照 → panic（内核 bug）
       - #DF: 打印双故障栈 → panic（不可恢复）
    3. 恢复执行或终止
```

#### 中断上下文约束

- **中断上下文不可睡眠**——不能调用 `Mutex`（可能 schedule），只能用 `Spinlock`；
- **中断嵌套**：普通中断可嵌套（IF flag 自动清除→重开），NMI/DF/MC 使用 IST 栈不嵌套；
- **下半部**：耗时操作（网络包处理、磁盘 I/O 回调）延迟到 softirq 队列在开中断后处理，避免中断关闭时间过长。

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
- 第一版**单核**，SMP 留到稳定版（避免 per‑CPU 复杂度吃掉内存预算）；
- **RT 预留（§19.1）**：调度器从第一天按"RT 类（优先级 + 抢占）+ 普通类（CFS/vruntime）"双队列分层设计；第一版只实现普通类，但 `SchedEntity` 与运行队列结构须能容纳 RT 类，避免后期重构。

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

### 6.1 总体对比

| 维度 | 裁剪后主线 Linux（参考） | Novos‑OS |
|---|---|---|
| 空闲常驻 | 50–80 MB | **≤ 32 MB** |
| 兼容层 | 数十年 ABI/驱动包袱 | 无，只保留所需子集 |
| 驱动数 | 成百上千（模块化） | 最小集（virtio/uart/timer） |
| namespace/cgroup | 完整但历史包袱多 | 全新实现，v2 单层 |
| 网络栈 | 完整 + 大量扩展 | 完整 TCP/IP，按需裁剪 |
| 开发语言 | C + 汇编 | **Rust（内存安全）** |
| 可审计性 | 极难 | 预算明确、可测可回归 |

### 6.2 子系统级简化（Linux 复杂 → Novos‑OS 简化）

以下逐子系统分析 Linux 的设计复杂度来源，以及 Novos‑OS 在保持容器宿主功能完整的前提下可以砍掉什么。

#### ① VFS 层

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| `file_operations` ~30 个回调 | 兼容数十种文件系统 | 精简到 ~10 个（read/write/lookup/create/unlink/readdir/stat/mmap） | 只支持 ramfs/tmpfs/overlayfs 三种 fs |
| `inode_operations` ~15 个回调 | 包含 ACL/xattr/fiemap/... | 去掉 ACL、xattr（第一版）、fiemap | 容器场景不依赖 ACL/xattr |
| POSIX/BSD 文件锁（flock/fcntl） | 完整文件锁语义 | **最小字节区间记录锁**（`F_SETLK/F_GETLK/F_UNLCK`，仅 SQLite 需求） | SQLite 需要记录锁防并发写坏库；`flock` 整文件锁不实现 |
| `address_space` + page cache 通用框架 | 为磁盘 fs 设计的页缓存机制 | tmpfs 用匿名页直接映射，不走 page cache | tmpfs/ramfs 文件本就在内存中 |
| `getdents` + 线性目录 | 兼容 ext2 线性目录 | BTreeMap 排序目录 | 内存 fs 天然有序 |
| 挂载传播（mount propagation） | 4 种传播类型 | 仅支持 private + bind | 容器 pivot_root 不需要共享传播 |
| VFS dentry 操作 20+ 个 | `d_delete/d_dentry/d_iput/d_compare/...` | 去掉 `d_compare`（统一 strcmp）、`d_dentry`、`d_iput` 等 | 只有 3 种 fs，不需要多态 |

> **收益估算**：VFS 层逻辑代码从 Linux 的 ~1.5 万行 → Novos‑OS 目标 ~2300 行（含最小记录锁 ~300 行），节省约 1–1.5MB .text。

#### ② 内存管理

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| Per-CPU 页分配器（pageset） | 多核减少锁竞争 | **去掉**，单 buddy + 自旋锁 | 单核无竞争 |
| LRU 多链表（active/inactive/unevictable） | 精细化回收 | 单一 LRU + shrink_target | 缓存总量小，不需要精细分级 |
| kswapd 后台回收线程 | 周期性异步回收 | **不实现**，同步 shrink 在 `memory.high` 触发 | 32MB 内核缓存少，同步即可 |
| Swap（swap cache + swap_map） | 磁盘交换 | **不实现**（已定） | 容器场景 OOM-kill > swap |
| THP（透明大页） | 减少 TLB miss | 仅内核直接映射用 2MB 大页；用户态 4K | 32MB 预算内 THP 收益有限 |
| 内存碎片整理（compaction） | 为高阶分配腾出连续页 | **不实现**，靠 buddy 合并 | 连续大页需求少 |
| OOM killer 复杂评分 | cgroup + 全局 + 优先级综合 | Cgroup 级 OOM-kill（杀容器内最重进程），全局兜底简化 | 容器隔离后 OOM 范围天然缩小 |
| 多种 GFP 标志（GFP_KERNEL/GFP_HIGHUSER/...） | 区分分配上下文 | 精简到 2 种：KERNEL（不可失败）、USER（可失败→OOM） | 上下文简单 |
| memcg 多层嵌套统计 | v1→v2 兼容 | v2 单一层级，只统计 anon/file/kernel | 无 v1 包袱 |

> **收益估算**：mm 层逻辑从 Linux ~2.5 万行 → Novos‑OS 目标 ~3000 行，节省约 1.5–2MB .text。

#### ③ 网络栈

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| Netfilter 框架（5 个 hook 点 × N 个 table） | iptables/nftables 通用包过滤 | 内联防火墙规则（线性表，`{dev,src,dst,sport,dport,action}`） | 容器网关规则少，不需要通用 hook |
| sk_buff 复杂指针运算（`skb->data/skb->head/skb->tail`） | 支持多种协议层 push/pull | 简化 Skb：`data: Vec<u8>` + 协议头偏移索引 | 容器场景协议种类少（IP/TCP/UDP/ARP） |
| qdisc / tc（流量整形） | HTB/TBF/PRIO 等 | **不实现**，单队列 FIFO | 第一版不追求 QoS 精细控制 |
| xfrm / IPsec | 加密隧道 | **不实现** | 容器网关不需要内核 IPsec（用户态 wireguard 可选） |
| 多协议路由（FIB trie + 多路由表 + policy routing） | 支持多表/策略路由 | 单路由表 + 最长前缀匹配 | 每个命名空间一张表，够用 |
| conntrack 模块化 | 支持 100+ 协议状态跟踪 | 只跟踪 TCP/UDP/ICMP 三种 | 网关场景只此三种 |
| 网络设备框架（net_device + NAPI + GRO/GSO） | 高吞吐优化 | 简化 NetDevice：rx_queue/tx_queue + 轮询中断 | 单核 + 32MB，不需要 NAPI 复杂度 |
| socket 层多地址族 | AF_INET/AF_INET6/AF_UNIX/AF_NETLINK/... | AF_INET + AF_UNIX + AF_INET6（后续） | 容器只需这三族 |

> **收益估算**：网络栈从 Linux ~10 万行 → Novos‑OS 目标 ~8000 行，节省约 1.5–2.5MB .text（但 TCP 状态机本身不可省，这是代码量最大的子系统）。

#### ④ 调度器

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| 多调度类（Stop/DL/RT/Fair/Idle） | 实时 + 公平 + 空闲 | 单一调度类（CFS 简化），无 RT/DL | 容器场景不需要内核级实时调度 |
| nice 级别（-20~+19，40 级） | 细粒度优先级 | 去掉 nice，仅用 cgroup `cpu.weight` | 容器用 cgroup 隔离 CPU 足够 |
| Per-CPU 运行队列 | 多核负载均衡 | **单运行队列** | 单核 |
| 负载均衡 / NUMA balancing | 跨核迁移 | **不实现** | 单核 |
| 调度域 / 调度组 | 层级化拓扑 | **不实现** | 单核 |
| RT 调度器（FIFO/RR） | 实时进程 | **不实现**（第一版） | 容器无实时需求 |
| DL 调度器（EDF） | 截止时间调度 | **不实现** | 同上 |

> **收益估算**：调度器从 Linux ~1 万行 → Novos‑OS 目标 ~800 行，节省约 0.5–1MB .text。

#### ⑤ 设备模型 / 驱动框架

> **架构骨架（§19.1）**：第一版就定型 **bus→device→driver 统一框架 + BSP（板级包）+ 中断分发**；真实设备外设（GPIO/I2C/SPI/CAN/多路 UART/PWM/ADC）五花八门，驱动模型不在第一版定型，每加一个外设推一次架构。

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| kobject + kset + sysfs | 设备层次化 + 用户态可见 | **不实现** sysfs | 容器管理靠 /proc 只读视图 |
| udev / devtmpfs | 动态设备节点 | 静态设备文件 + 驱动注册表 | 设备固定（virtio/uart/timer），新增走 bus→device→driver |
| 驱动模块加载（request_module） | 动态加载 | **不实现**，静态链接 | 内核裁掉不需要的代码 |
| 通用 DMA 框架 | 各种总线/设备差异 | 仅 virtio DMA（直接 MMIO）；DMA 抽象留接口（§13.3 Hal trait） | 只支持 virtio |
| 通用中断框架（irq_desc/chip/domain） | 中断控制器虚拟化 | 固定中断向量表 + **中断分发抽象**（外设 IRQ → 驱动回调） | IRQ 来源固定（timer/uart/virtio），分发框架留扩展 |

> **收益估算**：设备框架从 Linux ~3 万行 → Novos‑OS 目标 ~1500 行（含 bus→device→driver 框架），节省约 1–1.5MB .text。

#### ⑥ 时间与定时器

> **架构骨架（§19.1）**：第一版就定型 **通用时钟源抽象**（不同硬件 timer/RTC 差异屏蔽）+ **RTC（硬件实时时钟）** + **monotonic 时钟**；很多用户态程序依赖 monotonic 语义。

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| hrtimer 框架 + 红黑树 + 高精度 | 多种时钟分辨率 | 单一定时器堆（最小堆，tick 分辨率）+ 时钟源 trait | 32MB 预算内够用 |
| 多时钟类型（REALTIME/MONOTONIC/BOOTTIME/TAI/...） | POSIX 时钟全集 | 仅 MONOTONIC + REALTIME + RTC | 容器不需要 TAI/BOOTTIME |
| timer wheel（低精度）+ hrtimer（高精度）双框架 | 兼容旧 API + 新 API | 单一定时器堆 | 无旧 API 包袱 |
| tickless / NOHZ | 省电 | **不实现**（第一版） | 服务器场景不需要省电（嵌入式省电 = idle 指令级，见 §19.2 电源管理） |

> **收益估算**：时间子系统从 Linux ~5000 行 → Novos‑OS 目标 ~800 行（含时钟源抽象 + RTC），节省约 0.3–0.5MB .text。

#### ⑦ 进程创建（clone/fork）

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| `copy_process` ~20 个子步骤（copy_files/copy_fs/copy_mm/copy_sighand/copy_signal/copy_namespaces/copy_creds/...） | 每个子系统有独立 clone 回调 | Task 字段用 `Arc` 共享 → clone = `Arc::clone` + 按需 COW | Rust 所有权天然处理共享/独占语义 |
| `clone` flags 数十个 | 细粒度控制 | 精简到 5 种（NEWPID/NEWNET/NEWNS/NEWUTS/NEWUSER） | 容器只需这几种 |
| COW 页表标记流程 | 手动 `page->refcount` 管理 | `Arc<PageFrame>` 引用计数自动管理 | Rust 保证无泄漏/无 double-free |
| `do_fork → copy_process → wake_up_new_task` 链 | 分层复杂 | 单一 `fork()` 函数 | 无历史中间层 |

> **收益估算**：进程管理从 Linux ~1.5 万行 → Novos‑OS 目标 ~1500 行，节省约 0.8–1.2MB .text。

#### ⑧ 锁与并发

| Linux 设计 | 复杂度来源 | Novos‑OS 简化 | 安全性论证 |
|---|---|---|---|
| RCU（Read-Copy-Update） | 多核无锁读路径 | **不实现**（第一版用 defer 队列替代） | 单核无 RCU 需求 |
| seqlock | 读多写少无锁 | **不实现** | 单核 RwLock 足够 |
| per-CPU 变量 | 免锁快速路径 | **不实现** | 单核 |
| lockdep（运行时锁依赖检测） | 动态死锁检测 | 编译期 Rust 类型约束（`Send`/`Sync`） | 类型系统部分替代 |
| 多种锁变体（spinlock/rwlock/rwsem/percpu_rwsem/...） | 场景优化 | 精简到 3 种（Spinlock/Mutex/RwLock） | 3 种覆盖所有场景 |
| lock_ordering 靠人记 | 无编译期保证 | 可用 Rust 幻影类型（PhantomData）编码锁层级 | 从"靠人记"到"编译器检查" |

> **收益估算**：锁框架从 Linux ~8000 行 → Novos‑OS 目标 ~600 行，节省约 0.3–0.5MB .text。

### 6.3 设计提升（不仅简化，而是从根本上更好）

以下不是"砍掉 Linux 的东西"，而是利用从零编写 + Rust 语言优势，在设计层面做出**比 Linux 更好的选择**。

#### 提升①：Arc/Weak 替代手动 kref

```c
// Linux：手动引用计数，容易漏 put 导致泄漏，多 put 导致 double-free
struct kref ref;
kref_get(&ref);   // 忘记 → use-after-free
kref_put(&ref, cleanup);  // 多调 → double-free

// Novos‑OS：编译器保证
let task: Arc<Task> = Arc::new(...);
let weak = Arc::downgrade(&task);  // 破环
// Arc::clone → 引用计数 +1（编译器保证配对）
// 离开作用域 → 自动 drop → 计数 -1
// 计数归零 → 自动释放
```

> Linux 每年都有 kref 相关 CVE。Rust 编译器在编译期消除这一整类 bug。

#### 提升②：Result<T, E> 统一错误传播

```c
// Linux：错误传播靠人检查返回值，容易遗漏
int ret = do_something();
if (ret < 0) goto out;  // 忘记 goto → 资源泄漏
// 指针错误：IS_ERR(ptr) + PTR_ERR(ptr)，混合两种错误表示

// Novos‑OS：? 强制处理
fn do_something() -> Result<(), KernelError> { ... }

fn caller() -> Result<(), KernelError> {
    do_something()?;  // 忘记 ? → 编译错误
    Ok(())
}
```

#### 提升③：锁层级编译期编码

```rust
// 用幻影类型编码锁获取顺序，违反顺序 → 编译失败
struct LockLevel<const N: usize>;

// mm lock = Level<1>，fs lock = Level<2>，net lock = Level<3>
// 编译器保证只能从低 level → 高 level 获取
// 尝试反向 → 类型不匹配 → 编译失败
fn example(mm: &Lock<1>, fs: &Lock<2>) {
    let _fs_guard = fs.lock();  // OK: 1 → 2
    let _mm_guard = mm.lock();  // 编译错误：违反顺序 2 → 1
}
```

> Linux 靠 lockdep 运行时检测死锁（需要先触发才能发现），Rust 类型系统在编译期阻止。

#### 提升④：内核对象生命周期类型安全

```rust
// Linux：task_struct 生命周期靠 manual get/put，跨线程传递靠裸指针
struct task_struct *task = find_task_by_pid(pid);
put_task_struct(task);  // 忘记 → 泄漏

// Novos‑OS：Arc<Task> 自动管理
let task: Arc<Task> = find_task_by_pid(pid)?;
// 自动 drop，无泄漏风险

// Inode → Dentry 用 Weak 破环，编译器验证无环引用
struct Inode {
    dentry: Weak<Dentry>,  // 弱引用，不阻止回收
}
struct Dentry {
    inode: Arc<Inode>,    // 强引用，持有所有权
}
```

#### 提升⑤：内存预算 CI 强制断言

```yaml
# Linux：没有"子系统内存预算"概念，内核常驻靠运行时 slubtop 估算
# Novos‑OS：每个子系统 used/limit 是一等公民，CI 强制断言
- name: memory regression
  run: |
    # 断言内核常驻 ≤ 32MB
    USED=$(cat /proc/meminfo | grep kernel_used | awk '{print $2}')
    test $USED -le 33554432  # 32MB = 33554432 bytes
```

> Linux 的内存问题往往是"不知道哪个子系统在涨"。Novos‑OS 的预算模型让每个子系统的内存占用**可测、可断言、可回归**。

#### 提升⑥：unsafe 代码审查边界

```rust
// Linux：全部 C 代码都是"unsafe"，无法区分安全边界
// Novos‑OS：unsafe 块显式标注，可审查范围极小
pub fn buddy_alloc(order: u8) -> Result<PhysFrame, KernelError> {
    // 安全代码：逻辑正确性由 Rust 保证
    let area = find_free_area(order)?;
    
    // unsafe 块：仅此处有裸指针/硬件操作
    // SAFETY: area 已验证为有效空闲区域，order ≤ MAX_ORDER
    unsafe {
        let ptr = area.as_ptr();
        (*ptr).flags |= BUDDY_FLAG;
        (*ptr).refcount = 1;
    }
    
    Ok(PhysFrame { area })
}
```

> Novos‑OS 中 `unsafe` 占比目标 <5%，审查者只需集中看 `unsafe` 块。Linux 等于审查 100% 代码。

### 6.4 简化汇总与内存收益

| 子系统 | Linux 代码行（估） | Novos‑OS 目标（估） | .text 节省 |
|---|---|---|---|
| VFS + 文件系统 | ~15,000 | ~2,000 | 1–1.5 MB |
| 内存管理 | ~25,000 | ~3,000 | 1.5–2 MB |
| 网络栈 | ~100,000 | ~8,000 | 1.5–2.5 MB |
| 调度器 | ~10,000 | ~800 | 0.5–1 MB |
| 设备/驱动框架 | ~30,000 | ~1,000 | 1–1.5 MB |
| 时间/定时器 | ~5,000 | ~500 | 0.3–0.5 MB |
| 进程管理 | ~15,000 | ~1,500 | 0.8–1.2 MB |
| 锁/并发 | ~8,000 | ~600 | 0.3–0.5 MB |
| **合计** | **~208,000** | **~17,400** | **~7–10 MB** |

> 这 7–10MB 的 .text 节省，是 Novos‑OS 能把内核常驻压到 32MB 的核心来源——不靠编译选项，靠**设计上就不需要那些代码**。

---

## 7. 信号处理

### 7.1 信号投递模型

```
send_signal(target, signo):
    1. 在 target.signals.pending 中置位（位图，对应 1–31 标准信号）
    2. 若信号未被阻塞 → 标记 target 为 TIF_SIGPENDING
    3. 下次从内核态返回用户态时检查 TIF_SIGPENDING → do_signal()

do_signal():
    1. 从 pending 位图中取一个未阻塞信号
    2. 若注册了 handler：
       · 在用户栈上建 sigframe（保存寄存器 + signal mask）
       · 设 RIP = handler 地址，RDI = signo
       · 返回用户态执行 handler
    3. handler 执行完毕 → sigreturn 系统调用 → 恢复 sigframe → 回到中断点
    4. 若未注册 handler：
       · SIGKILL/SIGSTOP → 强制终止/暂停（不可捕获）
       · 其他 → 默认动作（通常终止进程）
```

### 7.2 信号与容器的交互

- 信号在 **pid namespace** 内投递：宿主向容器 PID 1 发信号时，按 ns 内编号找到 Task；
- `SIGCHLD`：容器内子进程退出 → init 收到 SIGCHLD → `waitpid` 回收僵尸；
- Cgroup OOM-kill：内核直接向容器内内存超限进程发 `SIGKILL`，同时向容器 init 发 `SIGKILL` 通知（通过 eventfd 或预留信号通道）。

### 7.3 信号结构

```rust
pub struct SignalState {
    pub pending: u32,              // 待处理信号位图
    pub blocked: u32,              // 被阻塞的信号掩码
    pub handlers: [SignalHandler; 32],  // 每信号一个 handler
}

pub enum SignalHandler {
    Default,                       // 默认动作
    Ignore,                        // SIG_IGN
    User { addr: VirtAddr, mask: u32, flags: u32 }, // 自定义 handler
}
```

---

## 8. 错误处理与恢复策略

### 8.1 错误分级

| 级别 | 来源 | 处理策略 |
|---|---|---|
| **用户态错误** | syscall 参数校验、缺页、非法地址 | 返回 `errno`，用户进程自行处理或被信号终止 |
| **可恢复内核错误** | 资源分配失败、缓存 shrink 失败 | `Result<T, KernelError>` 传播，降级服务（如关闭连接、丢弃包） |
| **不可恢复内核错误** | 断言失败、Double Fault、null ptr 解引用 | `panic!` → 打印寄存器/栈/调用链 → 停机（第一版无 kexec） |
| **容器 OOM** | Cgroup `memory.max` 超限 | kill 容器内最重进程（oom_score），不影响宿主 |
| **全局 OOM** | 内核自身内存不足 | 按优先级 kill 用户进程释放内存，最后兜底 kill 容器 |

### 8.2 panic 处理

```rust
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // 1. 关中断（cli），防止 panic handler 被中断
    // 2. 输出到串口：
    //    PANIC at kernel/src/mm/buddy.rs:142
    //    message: buddy free: double free on page 0x1f000
    //    registers: rax=... rbx=... rip=... cr2=...
    //    stack trace:
    //      buddy_free+0x42
    //      kfree+0x1a
    //      slab_cache_shrink+0x88
    //      ...
    // 3. 进入 halt 循环，等待调试器或重置
    loop { unsafe { asm!("hlt"); } }
}
```

- panic 信息走串口（UART），不走 VFS（防止文件系统也损坏时雪上加霜）；
- **第一版 panic 即停机**，不做 kexec/kdump（预算内放不下）；
- 稳定版可加 watchdog → 自动重启（M9 评估）。

### 8.3 Rust 错误传播

```rust
/// 内核统一错误类型
#[derive(Debug)]
pub enum KernelError {
    OutOfMemory,       // 分配失败
    InvalidArgument,   // syscall 参数非法
    NotFound,          // 资源不存在（文件/路由/inode）
    PermissionDenied,  // 权限不足
    WouldBlock,        // 非阻塞 IO
    BrokenPipe,        // 管道对端关闭
    AddrInUse,         // 端口/地址已占用
    Internal,          // 内核内部错误（不应出现，出现即 bug）
}

/// syscall 返回值映射
impl KernelError {
    pub fn to_errno(&self) -> i32 {
        match self {
            Self::OutOfMemory => -12,      // ENOMEM
            Self::InvalidArgument => -22,  // EINVAL
            Self::NotFound => -2,          // ENOENT
            Self::PermissionDenied => -1,  // EPERM
            Self::WouldBlock => -11,       // EAGAIN
            Self::BrokenPipe => -32,      // EPIPE
            Self::AddrInUse => -98,       // EADDRINUSE
            Self::Internal => -516,       // EIO（内部错误映射为 I/O）
        }
    }
}
```

设计要点：
- syscall 层统一 `Result<isize, KernelError>` → 返回值/errno 二选一；
- 内核内部不 panic、不 unwrap，全部 `?` 传播；
- 仅 `unsafe` 区段的可证不变量用 `assert!`，失败即 panic（设计正确则不应触发）。

---

## 9. 安全模型

### 9.1 权限模型

Novos‑OS 采用简化的 Unix 权限模型：

| 概念 | 实现 |
|---|---|
| **uid/gid** | Task 持有 `uid/gid/euid/egid`，root (uid=0) 绕过权限检查 |
| **capability** | 第一版不做细粒度 capability；root/非root 二分（后续可加） |
| **文件权限** | Inode.mode 的 rwxrwxrwx 位，open/lookup 时检查 |
| **容器隔离** | user namespace + uid 映射：容器内 root → 宿主非 root |

### 9.2 隔离保证

| 维度 | 隔离机制 | 容器逃逸面 |
|---|---|---|
| 进程 | pid namespace | 容器内只能看到 ns 内 PID |
| 文件 | mnt namespace + pivot_root | 容器看不到宿主 rootfs |
| 网络 | net namespace | 独立设备/路由/conntrack |
| 资源 | cgroup v2 | memory/cpu/pids 上限 |
| 用户 | user namespace | uid 映射，无宿主 root 权限 |
| IPC | ipc namespace | 独立 SysV IPC（第一版空实现，天然隔离） |
| 内核攻击面 | 最小 syscall 集 + `unsafe` 代码审查 | 从零内核，无历史漏洞 |

### 9.3 安全原则

1. **默认拒绝**——所有 syscall 默认返回 EPERM，显式放行才允许；
2. **容器内 root ≠ 宿主 root**——user namespace 映射保证；
3. **无 setuid**——第一版不实现 setuid/setgid 位（容器场景不需要）；
4. **地址空间随机化**——KASLR（内核随机化）和 ASLR（用户态 mmap 随机化）在 M9 评估加入。

---

## 10. 日志与可观测性

### 10.1 日志系统

```rust
#[macro_export]
macro_rules! log {
    ($level:expr, $($arg:tt)*) => {{
        if $level <= $crate::log::LOG_LEVEL {
            $crate::log::write($level, format_args!($($arg)*));
        }
    }};
}

// 便捷宏
log!(ERROR, "buddy: double free on page {:#x}", addr);
log!(WARN,  "tcp: retransmit timeout on port {}", port);
log!(INFO,  "container {} started (pid={})", name, pid);
log!(DEBUG, "dcache shrink: evicted {} entries", n);
log!(TRACE, "schedule: next={} vruntime={}", pid, vr);

pub static LOG_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
```

日志级别与用途：

| 级别 | 用途 | 生产环境默认 |
|---|---|---|
| ERROR | 不可恢复：panic 前兆、分配失败 | 开 |
| WARN | 可恢复异常：重传超时、OOM 候选 | 开 |
| INFO | 关键事件：容器创建/销毁、网络 up/down | 开 |
| DEBUG | 调试细节：shrink 路径、调度决策 | 关 |
| TRACE | 极细粒度：每次中断/调度 | 关 |

- 日志输出到串口（UART），第一版不做日志文件（省内存）；
- 可通过内核命令行参数 `log=debug` 调整级别；
- 日志带时间戳（tick 计数），便于排序和关联。

### 10.2 /proc 只读视图

```
/proc/
├── meminfo          # 内核内存使用（buddy/slab/各缓存 used/limit）
├── schedstat        # 调度统计（上下文切换次数、各 task vruntime）
├── net/             # 网络统计（设备流量/conntrack 条目/TCP 状态分布）
│   ├── dev          # 网络设备收发统计
│   ├── conntrack    # 连接跟踪表摘要
│   └── tcp          # TCP 状态机分布
├── cgroup/
│   └── memory       # 各 cgroup memory.current / memory.max
├── containers/      # 活跃容器列表 + 资源占用
└── uptime           # 内核运行时间
```

- `/proc` 完全只读（减少攻击面），不支持 `echo > /proc/...`；
- 预算监控数据从内核原子计数器直接读取，无额外内存开销。

### 10.3 内存预算看板

```rust
/// 内核全局内存统计（原子计数，/proc/meminfo 直接读取）
pub struct MemStat {
    pub total_pages: AtomicU64,        // 物理页总数
    pub free_pages: AtomicU64,         // 空闲页
    pub slab_used: AtomicU64,          // slab 已用（字节）
    pub dcache_entries: AtomicU64,     // dcache 条目数
    pub dcache_bytes: AtomicU64,       // dcache 占用（字节）
    pub sk_buff_count: AtomicU64,     // sk_buff 池大小
    pub sk_buff_bytes: AtomicU64,     // sk_buff 占用
    pub page_table_bytes: AtomicU64,  // 页表占用
    pub kernel_used: AtomicU64,       // 估算内核常驻总计
}
```

- `kernel_used` = `slab_used` + `dcache_bytes` + `sk_buff_bytes` + `page_table_bytes` + `.bss/.data` 静态大小；
- CI 内存断言直接读取此值与 32MB 比较；
- 每个 shrink 路径完成后更新对应计数器。

---

## 11. SMP 演进路线

### 11.1 第一版：单核（UP）

- 单一运行队列，`Spinlock` 保护，无需 IPI；
- 优势：无 per-CPU 数据结构，无 RCU，内存开销最低；
- 调度器、中断、缓存全部单核设计，简单可测。

### 11.2 稳定后评估（M9+）

| 能力 | 内存成本 | 是否值得加 |
|---|---|---|
| Per-CPU 运行队列 | +0.3–0.5MB | 2 核以上值得 |
| IPI（核间中断） | <0.1MB | 基础设施 |
| RCU 简化（defer 队列→多核） | +0.2–0.5MB | 取代 spinlock 热路径 |
| Per-CPU page cache | +0.5–1MB | 提高缓存命中率 |
| 内核栈 per-CPU 中断栈 | +0.1MB | 嵌套安全 |

**评估门槛**：SMP 的总内存增量不打破 32MB 预算。若超出，优先保证 UP 稳定。

### 11.3 SMP 演进顺序

```
UP 稳定 (M9)
  │
  ├─ 1. 加 IPI + per-CPU 运行队列（2 核验证）
  ├─ 2. 热路径 Spinlock → RCU（dcache/sk_buff 读路径）
  ├─ 3. per-CPU page/slab cache
  └─ 4. 扩展到 4 核 + IRQ 亲和性
```

---

## 12. 测试架构

### 12.1 测试分层

```
┌─────────────────────────────────────────────┐
│           CI (GitHub Actions)                │
│  ┌─────────┐  ┌──────────┐  ┌───────────┐  │
│  │  cargo  │  │   QEMU   │  │   内存    │  │
│  │  test   │  │ 集成断言  │  │  回归测试  │  │
│  └─────────┘  └──────────┘  └───────────┘  │
└─────────────────────────────────────────────┘
       │              │              │
       ▼              ▼              ▼
┌────────────┐ ┌────────────┐ ┌────────────┐
│  单元测试   │ │ 集成测试   │ │ 回归测试   │
│ (host 跑)  │ │ (QEMU 跑)  │ │ (CI 断言)  │
└────────────┘ └────────────┘ └────────────┘
```

### 12.2 单元测试（host）

逻辑与硬件解耦的子系统在 `cargo test` 中直接测试：

| 模块 | 测试内容 |
|---|---|
| buddy | 分配/释放/合并、多阶分裂、无泄漏/无重叠 |
| slab | 对象复用、partial 回收、size 阶梯正确性 |
| rbtree | 插入/删除/最左查询/平衡性 |
| 路径解析 | 路径分量拆分、挂载点跳转、dcache 命中/miss |
| TCP 状态机 | 三次握手→ESTABLISHED→四次挥手→TIME_WAIT 转换正确性 |
| Cgroup 记账 | page_charge/uncharge、memory.max 触发 |
| Namespace 层级 | pid ns 父子链、跨 ns 可见性 |

原则：内核类型用 `#[cfg(test)]` 注入 trait mock，测试不依赖真实硬件。

### 12.3 集成测试（QEMU）

```
qemu-system-x86_64 \
  -kernel build/novos.bin \
  -m 64M                    # 仅给 64MB 物理内存，逼出小内核
  -serial stdio \
  -nographic \
  -device virtio-net-device,netdev=net0 \
  -netdev user,id=net0

# 串口输出中断言：
# assert("boot ok")
# assert(init started)
# assert(shell ready)
# assert(/proc/meminfo kernel_used < 32MB)
```

### 12.4 回归测试

- **启动 N 容器后断言** `kernel_used ≤ 32MB`；
- **长跑 7 天** `kernel_used` 不单调爬升（无泄漏）；
- **shrink 压力**：填充 dcache 到上限 → 触发 shrink → 确认 `dcache_bytes` 回落；
- **网络压力**：1000 连接 burst → 确认 `sk_buff_bytes` 不无限增长；
- 回归测试结果记录到 `docs/bench/<date>/`，每次 PR 跑全量。

---

## 13. 扩展性设计（Linux 兼容容器宿主）

### 13.0 问题背景

原始定位（§1）是"最小容器宿主内核"——只跑 musl 静态链接的 ELF，只有 ramfs/tmpfs/overlayfs。

扩展目标：**Linux 兼容的容器宿主内核**——支持 ext4 磁盘文件系统、Docker 完整生态、`apt install` 包管理、JVM 和 Python 动态语言运行时。

这四个目标对内核有硬性要求，逐项分析：

| 用户目标 | 硬性内核需求 | 当前设计是否覆盖 |
|---|---|---|
| **ext4 磁盘文件系统** | Block I/O 层 + ext4 驱动 + 完整 VFS 操作 | ❌ 无 Block 层 |
| **Docker 完整生态** | OCI runtime + seccomp + 38 种 capabilities + devpts + CNI 网络 + 完整 /proc | ❌ 缺 seccomp/capability/devpts |
| **apt install** | 动态链接 + FHS 目录结构 + HTTPS 下载 + tar/gzip 解压 | ❌ 无动态链接 |
| **JVM 运行** | 动态链接 + futex + TLS + 完整信号 + getrandom + /proc/self/maps + 大页 mmap | ❌ 缺 futex/TLS/getrandom |
| **Python 运行** | 动态链接 + 部分信号 + getrandom + /proc/self/exe | ❌ 缺动态链接/getrandom |

> **结论**：四个目标中 3 个需要**动态链接**，2 个需要 **futex**，2 个需要 **Block I/O 层**。这三个是最大的架构缺口。

### 13.1 需求到架构的映射

```
用户目标                          架构扩展点
──────────                        ──────────────────
ext4 文件系统 ──────────────────▶ ① VFS 可扩展文件系统框架
                  ─────────────▶ ② Block I/O 层（bio + 驱动 trait）
                  ─────────────▶ ③ Page Cache（文件页缓存）

Docker 生态 ───────────────────▶ ④ 设备文件框架（devtmpfs/devpts/char）
        ───────────────────────▶ ⑤ Capabilities（38 种 Linux cap）
        ───────────────────────▶ ⑥ Seccomp BPF
        ───────────────────────▶ ⑦ 完整 /proc（/proc/self/* 等）
        ───────────────────────▶ ⑧ 网络扩展（veth/bridge/CNI）

apt install ───────────────────▶ ⑨ 动态链接（ELF .dynamic + GOT/PLT + ld.so）
            ───────────────────▶ ⑩ HTTPS/TLS（用户态，内核需支持 TLS socket）

JVM/Python ────────────────────▶ ⑨ 动态链接（共享库 mmap）
           ───────────────────▶ ⑪ futex（pthread 基础设施）
           ───────────────────▶ ⑫ TLS（FS/GS 段基址）
           ───────────────────▶ ⑬ getrandom + /dev/urandom
           ───────────────────▶ ⑭ 完整信号（sigaction/sigprocmask/sigaltstack）
           ───────────────────▶ ⑮ timerfd / signalfd（事件循环）
```

以下逐个设计这些扩展点。

### 13.2 ① VFS 可扩展文件系统框架

当前设计（§3.6）的 `SuperBlock` 只有 `fs_type: Arc<FsType>`，但没有定义注册机制。需要设计 trait-based 注册：

```rust
/// 文件系统驱动 trait——新增 ext4/procfs/sysfs 只需实现此 trait
pub trait FileSystemDriver: Sync + Send {
    fn name(&self) -> &'static str;           // "ext4" / "proc" / "sysfs"
    fn mount(&self, dev: Option<&BlockDevice>, opts: &MountOpts) -> Result<SuperBlock, KernelError>;
    fn kill_sb(&self, sb: &SuperBlock);       // 卸载时回收
}

/// 全局文件系统注册表（编译期填充）
pub static FS_REGISTRY: [(&'static str, &'static dyn FileSystemDriver); 8] = [
    ("ramfs",    &RamfsDriver),
    ("tmpfs",    &TmpfsDriver),
    ("overlay",  &OverlayDriver),
    ("proc",     &ProcfsDriver),       // §13.7
    ("sysfs",    &SysfsDriver),         // 最小化
    ("devtmpfs", &DevtmpfsDriver),      // §13.6
    ("devpts",   &DevptsDriver),        // §13.6
    ("ext4",     &Ext4Driver),           // §13.3
];

/// VFS 操作也需 trait 化，支持可选操作
pub trait InodeOps: Sync + Send {
    // 必选
    fn lookup(&self, name: &str) -> Result<Arc<Inode>, KernelError>;
    fn create(&self, name: &str, mode: FileMode) -> Result<Arc<Inode>, KernelError>;
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, KernelError>;
    fn write(&self, offset: u64, buf: &[u8]) -> Result<usize, KernelError>;
    
    // 可选——默认返回 ENOSYS
    fn xattr_get(&self, _name: &str) -> Result<Vec<u8>, KernelError> { Err(ENOSYS) }
    fn xattr_set(&self, _name: &str, _val: &[u8]) -> Result<(), KernelError> { Err(ENOSYS) }
    fn flock(&self, _cmd: FlockCmd) -> Result<(), KernelError> { Err(ENOSYS) }
    fn fiemap(&self) -> Result<Vec<Extent>, KernelError> { Err(ENOSYS) }
}
```

设计要点：
- 新增 FS 不改 VFS 核心——只加注册表条目 + 实现 trait；
- 可选操作用默认实现返回 ENOSYS，不强制每种 FS 都实现全部；
- ext4 可后续加入，不影响 ramfs/tmpfs/overlayfs 路径。

### 13.3 ② Block I/O 层 + ③ Page Cache

当前设计显式排除了 Block 层（§6.2 ⑤）。ext4 需要它。设计最小 Block 层：

```rust
/// 块设备 trait——virtio-blk 实现此 trait
pub trait BlockDevice: Sync + Send {
    fn read_block(&self, lba: u64, buf: &mut [u8; 4096]) -> Result<(), KernelError>;
    fn write_block(&self, lba: u64, buf: &[u8; 4096]) -> Result<(), KernelError>;
    fn flush(&self) -> Result<(), KernelError>;
    fn block_size(&self) -> usize { 4096 }
    fn nr_blocks(&self) -> u64;
}

/// BIO 请求（简化版，无 I/O 调度器）
pub struct Bio {
    pub dev: Arc<dyn BlockDevice>,
    pub lba: u64,
    pub len: usize,           // 扇区数
    pub buf: Vec<u8>,
    pub op: BioOp,             // Read | Write | Flush
    pub callback: Option<Box<dyn FnOnce(Result<(), KernelError>) + Send>>,
}

/// Block 层 = 简单队列 + 轮询/中断完成
pub struct BlockLayer {
    pub queue: VecDeque<Bio>,
    pub inflight: usize,
    pub lock: Spinlock,
}
```

设计要点：
- **无 I/O 调度器**（cfq/deadline/mq）——virtio-blk 单队列 + FIFO 即可；
- **同步优先 + 异步回调**——内核自身 I/O 同步等；用户态 page cache 回调填充；
- Block 层代码量目标 ~500 行（Linux ~2 万行）。

#### Page Cache（文件页缓存）

当前 tmpfs 用匿名页直接映射。ext4 的文件 mmap 需要文件页缓存：

```rust
/// 地址空间——文件到页的映射缓存（对应 Linux address_space）
pub struct AddressSpace {
    pub inode: Arc<Inode>,
    pub pages: BTreeMap<u64, Arc<PageFrame>>,  // 文件偏移 → 物理页
    pub nr_pages: AtomicU64,
    pub lock: RwLock,
}

impl AddressSpace {
    /// 文件 mmap 缺页时调用——从磁盘读页到缓存
    fn readpage(&self, offset: u64) -> Result<Arc<PageFrame>, KernelError> {
        if let Some(page) = self.pages.read().get(&offset) {
            return Ok(page.clone());  // cache hit
        }
        let mut page = alloc_page()?;
        let lba = offset / 4096;
        self.inode.dev.read_block(lba, page.as_mut())?;
        self.pages.write().insert(offset, page.clone());
        Ok(page)
    }
}
```

- MAP_SHARED 的 .so 文件多进程共享同一 AddressSpace → 同一物理页；
- Page cache 可被 shrink 回收（修改过的脏页先回写再释放）。

### 13.4 ④ 设备文件框架

当前只有 `/dev/uart`。Docker + JVM + Python 需要更多设备文件：

```rust
/// 字符设备 trait
pub trait CharDevice: Sync + Send {
    fn name(&self) -> &'static str;      // "null" / "zero" / "urandom"
    fn read(&self, buf: &mut [u8]) -> Result<usize, KernelError>;
    fn write(&self, buf: &[u8]) -> Result<usize, KernelError>;
    fn ioctl(&self, _cmd: u32, _arg: usize) -> Result<isize, KernelError> { Err(ENOTTY) }
}

/// devtmpfs——自动注册设备节点
pub struct Devtmpfs {
    pub devices: BTreeMap<&'static str, Arc<dyn CharDevice>>,
}

/// 标准字符设备
pub struct NullDev;        // /dev/null：read 返回 EOF，write 丢弃
pub struct ZeroDev;        // /dev/zero：read 返回 \0
pub struct UrandomDev;     // /dev/urandom：read 返回随机字节（RDRAND）
pub struct RandomDev;     // /dev/random：同 urandom（第一版不区分阻塞/非阻塞）

/// devpts——伪终端子系统（docker exec / SSH 需要 PTY）
pub struct Devpts {
    pub ptys: BTreeMap<u32, PtyPair>,  // pts/N → master/slave 对
}

pub struct PtyPair {
    pub master: Arc<PtyMaster>,   // /dev/ptmx 打开的端
    pub slave: Arc<PtySlave>,    // /dev/pts/N
    pub buf: RingBuf<u8>,         // 双向缓冲
}
```

设备清单：

| 设备 | 用途 | 谁需要 |
|---|---|---|
| `/dev/null` | 丢弃输出 | 所有程序 |
| `/dev/zero` | 填零 | JVM（初始化内存） |
| `/dev/urandom` | 随机数 | JVM SecureRandom、Python os.urandom |
| `/dev/ptmx` + `/dev/pts/N` | 伪终端 | docker exec、SSH、shell 交互 |
| `/dev/net/tun` | TUN/TAP 设备 | Docker 网络插件（可选） |

### 13.5 ⑤ Capabilities + ⑥ Seccomp

当前设计（§9.1）只有 root/非root 二分。Docker 需要完整的 Linux capability 模型：

```rust
/// Linux capabilities（bit-64 set，第一版实现常用的 ~15 种）
bitflags! {
    pub struct CapSet: u64 {
        const CAP_CHOWN          = 1 << 0;
        const CAP_NET_BIND_SERVICE = 1 << 10;  // 绑定 <1024 端口
        const CAP_NET_RAW        = 1 << 13;     // raw socket
        const CAP_SYS_ADMIN      = 1 << 21;     // pivot_root, mount, namespace
        const CAP_SYS_PTRACE     = 1 << 19;     // ptrace
        const CAP_KILL           = 1 << 5;      // kill 其他用户进程
        const CAP_NET_ADMIN      = 1 << 12;     // 网络配置
        const CAP_DAC_OVERRIDE   = 1 << 1;      // 绕过文件权限
        // ... 其余按需加
    }
}

pub struct TaskCreds {
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub caps_permitted: CapSet,   // 可用 cap 集
    pub caps_effective: CapSet,   // 当前生效 cap 集
    pub caps_inheritable: CapSet, // exec 继承集
    pub caps_bounding: CapSet,    // 上限
}
```

权限检查改为：
```rust
fn check_cap(task: &Task, cap: CapSet) -> bool {
    task.creds.euid == 0 || task.creds.caps_effective.contains(cap)
}
```

#### Seccomp BPF

Docker 默认 seccomp profile 过滤危险 syscall。需要最小化 BPF 解释器：

```rust
/// seccomp filter——BPF 字节码解释执行（仅 syscall number 过滤）
pub struct SeccompFilter {
    pub code: Vec<BpfInsn>,    // BPF 指令序列
    pub default_action: SeccompAction,  // ALLOW / KILL / ERRNO
}

pub enum SeccompAction {
    Allow,
    Kill,
    Errno(i32),
    Trace,  // 不实现，fallback to Kill
}

/// Task 持有可选 seccomp filter
pub struct Task {
    // ... 其他字段
    pub seccomp: Option<Arc<SeccompFilter>>,
}

/// syscall 入口检查
fn syscall_enter(task: &Task, nr: usize, args: &[usize]) -> Result<(), KernelError> {
    if let Some(filter) = &task.seccomp {
        match filter.eval(nr, args) {
            SeccompAction::Allow => Ok(()),
            SeccompAction::Kill => Err(KernelError::Killed),  // SIGKILL
            SeccompAction::Errno(e) => Err(KernelError::from_errno(e)),
        }
    } else {
        Ok(())
    }
}
```

设计要点：
- BPF 解释器只过滤 syscall number + 参数（不执行任意 BPF），代码量 <500 行；
- Docker 默认 profile 兼容：允许 ~300 个 syscall，禁止 ~50 个危险 syscall。

### 13.6 ⑨ 动态链接支持

这是最大的架构缺口。当前显式声明"第一版不做动态加载器"。JVM、Python、apt 全部需要。

#### 内核侧需求

```rust
/// ELF 动态段——内核需识别 PT_INTERP 和 PT_DYNAMIC
pub struct ElfDynamicInfo {
    pub interp: Option<String>,        // PT_INTERP: "/lib/ld-musl-x86_64.so.1"
    pub dynamic: Option<DynamicSection>, // PT_DYNAMIC: GOT/PLT/重定位信息
    pub needed: Vec<String>,            // DT_NEEDED: 依赖的 .so 列表
}

/// 内核 ELF 加载器扩展
fn load_elf(file: &File) -> Result<ProcessImage, KernelError> {
    let interp = elf.interp();  // 有 PT_INTERP = 动态链接
    
    if let Some(interp_path) = interp {
        // 1. 加载 ld.so（动态链接器）到地址空间
        let ld = load_elf_file(interp_path)?;
        // 2. 设置 AT_ENTRY 辅助向量 = ld.so 入口
        // 3. 设置 AT_BASE = ld.so 加载地址
        // 4. 设置 AT_PHDR/AT_PHNUM = 主程序 program header 信息
        // 5. 内核不解析 GOT/PLT——交给 ld.so 在用户态完成
        // 6. 跳转到 ld.so 入口，由 ld.so 加载所有 DT_NEEDED 库
    } else {
        // 静态链接——原路径不变
    }
}
```

#### 共享库 mmap

```rust
/// MAP_SHARED + 文件映射——.so 文件多进程共享物理页
pub fn mmap(addr: usize, len: usize, prot: Prot, flags: MapFlags, fd: u32, offset: u64) 
    -> Result<usize, KernelError> 
{
    match (flags.contains(MAP_SHARED), fd) {
        (true, Some(fd)) => {
            // 文件共享映射——走 AddressSpace page cache（§13.3）
            let inode = file.dentry.inode;
            let aspace = inode.address_space();
            // 多个进程映射同一 .so → 共享同一物理页
            map_pages(addr, len, prot, PageSource::File(aspace, offset))?;
        }
        (false, None) => {
            // 匿名私有——原路径（匿名页 + COW）
        }
        _ => { /* 其他组合 */ }
    }
}
```

#### 用户态需求

| 组件 | 实现方式 | 内存成本 |
|---|---|---|
| `ld-musl-x86_64.so.1` | musl 动态链接器 | ~200KB .text |
| `libc.so` | musl 共享库 | ~500KB .text |
| `libpython3.x.so` | Python 共享库 | ~2–4MB |
| `libjvm.so` | JVM 共享库 | ~10–20MB（不计入内核预算） |

> **关键**：.so 的内存不计入内核 32MB 预算——它们是用户态映射，走 Cgroup memory.max 隔离。内核只需支持**文件页缓存 + MAP_SHARED**。

#### 辅助向量（auxv）扩展

JVM/Python 编译时用了 stack protector，需要 AT_RANDOM：

```rust
/// 内核传递给用户态的辅助向量
pub enum AuxType {
    AT_PHDR = 3,      // program header 地址（动态链接必需）
    AT_PHENT = 4,     // program header entry size
    AT_PHNUM = 5,     // program header count
    AT_PAGESZ = 6,    // 页大小 4096
    AT_BASE = 7,      // ld.so 基地址（动态链接必需）
    AT_ENTRY = 9,     // 程序入口
    AT_RANDOM = 25,   // 16 字节随机数（stack canary 种子）
    AT_EXECFN = 31,   // argv[0]
}
```

### 13.7 ⑪ futex——pthread 基础设施

JVM 的 monitor（synchronized）、Python 的 GIL 都基于 pthread，pthread 的核心是 futex：

```rust
/// futex 系统调用
pub fn sys_futex(uaddr: usize, op: FutexOp, val: u32, timeout: Option<Duration>, 
                 uaddr2: usize, val3: u32) -> Result<i32, KernelError> {
    match op {
        FutexOp::Wait => {
            // 1. 验证 uaddr 是用户态可写地址
            // 2. 原子比较 *uaddr == val ? 不等则返回 EAGAIN
            // 3. 相等则将当前 task 挂到 uaddr 对应的等待队列
            // 4. schedule 切出
            // 5. 被唤醒后返回 0
        }
        FutexOp::Wake => {
            // 1. 从 uaddr 等待队列唤醒最多 val 个 task
            // 2. 返回唤醒数
        }
        FutexOp::Requeue => {
            // 从 uaddr 队列移 val 个到 uaddr2 队列（避免 thundering herd）
        }
    }
}

/// futex 等待队列——按物理页地址索引
pub struct FutexTable {
    pub table: HashMap<PhysAddr, WaitQueue>,  // 物理页地址 → 等待队列
    pub lock: Spinlock,
}
```

设计要点：
- 用**物理页地址**索引（而非虚拟地址），因为不同进程映射同一共享内存时虚拟地址不同但物理页相同；
- futex 代码量目标 ~200 行，但它是 pthread/JVM/Python GIL 的底层依赖。

### 13.8 ⑫ TLS（线程局部存储）

JVM、Python、glibc/musl 的 pthread 都依赖 TLS（`__thread`/`pthread_key`）：

```rust
/// x86-64 TLS 通过 FS 段基址实现
/// 设置 FS base MSR → 后续 %fs:offset 访问 TLS 区域

/// sys_set_thread_area / arch_prctl(ARCH_SET_FS, addr)
pub fn sys_arch_prctl(code: u32, addr: usize) -> Result<(), KernelError> {
    match code {
        ARCH_SET_FS => {
            // 设置 FS base = addr（通过 WRMSR 0xC0000100）
            unsafe { wrmsr(0xC0000100, addr as u64); }
            task.tls_base = addr;
        }
        ARCH_GET_FS => { /* 读取 FS base */ }
        ARCH_SET_GS => { /* GS base（X32 ABI） */ }
        ARCH_GET_GS => { /* ... */ }
    }
    Ok(())
}

/// Task 结构增加 TLS 字段
pub struct Task {
    // ... 其他字段
    pub tls_base: usize,  // FS 段基址（线程局部存储）
    pub clear_child_tid: Option<usize>,  // clone(CLONE_CHILD_CLEARTID) 用
}
```

- clone 时设置 `CLONE_SETTLS` → 新线程 TLS 区域；
- `pthread_create` 底层调 `clone(CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SETTLS)`；
- 上下文切换时恢复 FS base。

### 13.9 ⑬ getrandom + 熵源

```rust
/// sys_getrandom(buf, len, flags)——JVM SecureRandom / Python os.urandom
pub fn sys_getrandom(buf: usize, len: usize, flags: u32) -> Result<isize, KernelError> {
    let mut data = vec![0u8; len];
    fill_random(&mut data)?;
    copy_to_user(buf, &data)?;
    Ok(len as isize)
}

fn fill_random(buf: &mut [u8]) -> Result<(), KernelError> {
    // 第一版：RDRAND 指令（x86 硬件随机数）
    for chunk in buf.chunks_mut(8) {
        let val = unsafe { rdrand() };
        chunk.copy_from_slice(&val.to_le_bytes()[..chunk.len()]);
    }
    // 可选：混入 jiffies + PID + 网络包时戳（增加熵）
    Ok(())
}

/// /dev/urandom 也调 fill_random
impl CharDevice for UrandomDev {
    fn read(&self, buf: &mut [u8]) -> Result<usize, KernelError> {
        fill_random(buf)?;
        Ok(buf.len())
    }
}
```

### 13.10 ⑭ 完整信号

当前设计（§7）只有基本信号投递。JVM 需要：

| 需求 | JVM 用途 | 当前设计 |
|---|---|---|
| `sigaction` | 注册 handler（含 SA_SIGINFO） | 有基本版本，需加 SA_SIGINFO |
| `sigprocmask` | 阻塞/恢复信号 | 有 blocked 位图，需加 syscall |
| `sigaltstack` | 信号处理用独立栈（JVM 在信号栈中处理 SIGSEGV） | ❌ 缺 |
| `SIGSEGV` 捕获 | JVM 用 SIGSEGV 做 null 检查（试探写 → 捕获 → 处理） | 有 #PF handler，但需正确投递 SIGSEGV |
| `SIGPROF` | JVM 性能分析（定时采样） | 需 timerfd → SIGPROF 路径 |
| `signalfd` | 某些事件循环库用 | ❌ 缺（可选） |

扩展信号结构：

```rust
pub struct SignalState {
    pub pending: u64,              // 扩展到 64 位（支持实时信号 1–63）
    pub blocked: u64,
    pub handlers: [SigAction; 64], // 扩展到 64 个信号
    pub altstack: Option<SignalStack>,  // sigaltstack（JVM 必需）
}

pub struct SigAction {
    pub handler: VirtAddr,
    pub flags: SigFlags,    // SA_SIGINFO | SA_RESTART | SA_ONSTACK | SA_NODEFER
    pub mask: u64,
    pub restorer: Option<VirtAddr>,  // sigreturn 跳板
}

pub struct SignalStack {
    pub base: VirtAddr,
    pub size: usize,
    pub flags: SS_AUTODISARM,
}
```

### 13.11 ⑮ timerfd / signalfd + ⑩ 网络扩展

#### timerfd

JVM 和 Python 的事件循环（epoll + timerfd）：

```rust
/// timerfd_create / timerfd_settime
/// 本质是一个 fd，到期后可读（epoll 可监听）
pub struct TimerFd {
    pub timer: Arc<Timer>,
    pub overrun: AtomicU64,
    pub wait: WaitQueue,
}

// epoll 监听 timerfd：到期 → 可读 → epoll_wait 返回
```

#### 网络扩展（Docker CNI）

| 能力 | 用途 | 当前设计 | 扩展 |
|---|---|---|---|
| veth pair | 容器网络连接到 bridge | ❌ | 新增 veth 虚拟设备 |
| bridge | 容器间二层转发 | ❌ | 新增 bridge 虚拟设备 |
| iptables/NAT（完整） | Docker 端口映射 | 有基础 NAT | 增加 DNAT 端口映射规则 |
| /dev/net/tun | TUN/TAP 设备 | ❌ | 可选（部分 CNI 插件需要） |

```rust
/// veth 虚拟设备对——一端在容器 net ns，一端在宿主 bridge
pub struct VethPair {
    pub peer_a: Arc<NetDevice>,  // 容器侧
    pub peer_b: Arc<NetDevice>,  // 宿主侧（挂到 bridge）
}

/// bridge——二层转发
pub struct Bridge {
    pub ports: Vec<Arc<NetDevice>>,
    pub mac_table: HashMap<[u8; 6], Arc<NetDevice>>,  // MAC → 端口
}
```

### 13.12 ⑦ 完整 /proc 扩展

当前 /proc（§10.2）只有内存统计。JVM/Python/Docker 需要：

```
/proc/
├── meminfo              # 已有
├── cpuinfo              # 新增：CPU 型号/特性（JVM 检测指令集）
├── mounts               # 新增：挂载表（Docker 检查 rootfs）
├── filesystems          # 新增：支持的 FS 列表
├── self/                # 新增：当前进程视图
│   ├── maps             # ★ JVM/Python/gdb 必需：地址空间映射
│   ├── status           # ★ JVM 读取 RSS/PeakRSS
│   ├── exe              # ★ /proc/self/exe → 可执行文件符号链接
│   ├── fd/              # ★ fd 表目录（Docker 检查）
│   ├── cmdline          # 进程命令行
│   └── cgroup           # 所属 cgroup 路径
├── <pid>/
│   ├── maps             # ptrace / 调试用
│   ├── status
│   ├── exe
│   ├── fd/
│   └── cmdline
├── net/                 # 已有
│   ├── dev
│   ├── conntrack
│   └── tcp
└── cgroup/              # 已有
```

`/proc/self/maps` 格式（JVM/GDB 必需）：
```
00400000-00600000 r-xp 00000000 00:01 1234     /bin/java
7f0000000000-7f0000200000 r-xp 00000000 00:01 5678  /lib/ld-musl-x86_64.so.1
7f0000200000-7f0000400000 r-xp 00000000 00:01 9012  /lib/libjvm.so
7ffff0000000-7ffff0020000 rw-p 00000000 00:00 0     [stack]
7ffff7ffd000-7ffff7fff000 r-xp 00000000 00:00 0     [vdso]
```

### 13.13 架构变更总览

扩展后的分层架构（新增/变更的层用 ★ 标注）：

```
┌─────────────────────────────────────────────────────────────────┐
│  用户态  init │ shell │ containerd │ apt │ JVM │ Python │ gateway│
├─────────────────────────────────────────────────────────────────┤
│  系统调用层  ★ futex │ ★ getrandom │ ★ arch_prctl(TLS) │       │
│  ★ seccomp check │ ★ capability check │ 信号 │ fd/文件句柄     │
├─────────────────────────────────────────────────────────────────┤
│  调度器 │ VFS │ 网络栈 │ epoll │ ★ timerfd │ ★ signalfd        │
├─────────────────────────────────────────────────────────────────┤
│  进程/内存 │ Namespace │ Cgroup │ OverlayFS │ ★ Page Cache     │
├─────────────────────────────────────────────────────────────────┤
│  ★ Block I/O │ ★ devtmpfs │ ★ devpts │ ★ procfs │ ★ sysfs      │
│  ★ veth/bridge │ 驱动（virtio-net/blk/uart/timer）              │
├─────────────────────────────────────────────────────────────────┤
│  内核核心  x86‑64 启动 │ 中断/异常 │ 内存 │ ★ TLS(FS/GS) │ 时钟  │
└─────────────────────────────────────────────────────────────────┘
```

### 13.14 内存预算影响评估

| 扩展项 | .text 增量 | 运行期增量 | 计入 32MB? |
|---|---|---|---|
| Block I/O 层 | +0.3–0.5 MB | page cache 按需 | .text 计入，cache 可 shrink |
| ext4 驱动 | +0.5–1 MB | inode/dentry 缓存 | 可 shrink |
| devtmpfs/devpts | +0.1–0.2 MB | 设备对象 | 固定 |
| procfs 扩展（/proc/self/*） | +0.1–0.2 MB | 按需生成 | 极低 |
| futex | +0.1 MB | 等待队列 | 随连接释放 |
| Capabilities | +0.05 MB | 无 | — |
| Seccomp BPF | +0.1–0.2 MB | filter 对象 | 随进程释放 |
| 动态链接（ELF 动态段 + MAP_SHARED） | +0.3–0.5 MB | page cache | 可 shrink |
| getrandom | +0.05 MB | 无 | — |
| TLS（FS/GS + arch_prctl） | +0.05 MB | 无 | — |
| 信号扩展（sigaltstack + SA_SIGINFO） | +0.1 MB | sigframe | 随信号释放 |
| timerfd/signalfd | +0.1 MB | fd 对象 | 随 fd 关闭释放 |
| veth/bridge | +0.2–0.3 MB | 设备对象 | 随容器释放 |
| **合计** | **+2.1–3.3 MB** | | |

> **结论**：扩展后内核 .text 增加约 2–3MB。32MB 预算需要调整为 **36–40MB**（或通过 feature flag 控制是否编译动态链接/ext4/设备框架——不需要这些特性的场景仍保持 32MB）。

### 13.15 Feature Flag 策略

用 Cargo feature 控制扩展编译，满足两种部署场景：

```toml
# Cargo.toml
[features]
default = ["minimal"]           # 默认：最小容器宿主
minimal = []                     # 32MB：ramfs/tmpfs/overlayfs + 静态链接
full = [                         # 40MB：Linux 兼容容器宿主
    "block-layer",
    "ext4",
    "dynamic-link",
    "devtmpfs",
    "futex",
    "capabilities",
    "seccomp",
    "getrandom",
    "tls",
    "procfs-full",
    "timerfd",
    "cni-net",
]
```

```rust
// 代码中用 cfg 控制
#[cfg(feature = "dynamic-link")]
fn load_dynamic_elf(...) { ... }

#[cfg(not(feature = "dynamic-link"))]
fn load_dynamic_elf(...) { Err(ENOSYS) }  // 不支持时返回 ENOSYS
```

- `--no-default-features --features minimal` → 32MB 最小版；
- `--features full` → ~40MB Linux 兼容版；
- 同一份代码，两种部署形态。

---

## 14. 参考实现映射（外部借鉴整合）

> 完整调研见 `REFERENCES.md`（25 个 GitHub Rust 开源组件）。本节把这些参考的**可借鉴点落成设计决策**：每个条目 = 采纳什么、用到哪个子系统、对应本文档哪一节。
>
> 原则：**借鉴思路与接口设计，不引入外部依赖**——内核自包含、`no_std`，32MB/40MB 预算内不允许背大型外部 crate。

### 14.1 设计决策总表

| # | 设计决策（采纳） | 来源组件 | 落地位置 |
|---|---|---|---|
| 1 | 调度就绪队列用**侵入式红黑树**（哨兵 nil + O(1) 缓存最左节点），取 `vruntime` 最小者 = 取最左 | intrusive-collections / intrusive-red-black-tree | §4.2、§3.3 `SchedEntity` |
| 2 | 内核堆 slab 用 **SLUB 风格 bitmap 对象位图 + size class 阶梯**（free 链表只需存 index） | buddy-slab-allocator / slabmalloc | §3.1 `SlabCache` |
| 3 | buddy 全局分配接入 **OOM 回调**（堆耗尽 → 触发 shrink / cgroup OOM-kill，而非直接 panic） | buddy_system_allocator `LockedHeapWithRescue` | §5.2、§8.1 |
| 4 | `mmap`/页表映射返回**类型化句柄** `MappedPages`（VA↔PA 双射、读写/执行权限分离，杜绝"映射了忘记录/权限错配"） | Theseus | §3.2 `Mm/Vma` |
| 5 | 网络缓冲池设**编译期上限**（接口地址数、分片缓冲、重组缓冲、sk_buff 水位） | smoltcp feature 常量 | §2.1-B、§5.2、§3.8 |
| 6 | virtio 驱动通过 **Hal trait 抽象 DMA**（`dma_alloc/dma_dealloc/phys_to_virt/virt_to_phys`），与页分配器/cgroup 记账解耦 | virtio-drivers / rCore | §13.3 `BlockDevice`、TASKS M10-01 |
| 7 | VFS 驱动 trait 用**"必选 + 可选默认 ENOSYS"**模式，新增 FS 不改 VFS 核心 | ArceOS axfs_vfs | §13.2 `FileSystemDriver/InodeOps` |
| 8 | 容器生命周期状态机（created/running/stopped + OOM→SIGKILL 流程）与 libcontainer 语义对齐 | youki libcontainer | §4.6、§7.2、TASKS M14-06 |
| 9 | cgroup v2 控制器（memory/pids/cpu）行为与 libcgroups v2 语义对齐 | youki libcgroups | §3.5 |
| 10 | seccomp 采用 **filter_action（白名单）+ default_action 分离**语义，参数匹配可扩展（eq/ne/ge/lt/masked_eq） | seccompiler | §13.5 |
| 11 | ext4：解析结构对照 ext4-view-rs（`no_std` 只读），写路径/JBD2 语义对照 am-fs-ext4，测试磁盘镜像用 mkext4 | ext4-view-rs / am-fs-ext4 / mkext4 | §13.3、TASKS M10 |
| 12 | 定时器：第一版最小堆（确定性），评估期对照 tokio **6 层分层时间轮**（tick 粒度 + O(1) 入队） | tokio-time | §4.5 |
| 13 | futex 等待队列**按物理页地址哈希索引**（不同进程共享内存虚拟地址不同、物理页相同） | Rust std futex | §13.7 |
| 14 | ELF 动态加载 + auxv 初始化 + `ARCH_SET_FS` 对照 arceos-runlinuxapp 的加载器/用户栈帧模型 | arceos-runlinuxapp | §13.6/13.8、TASKS M11 |
| 15 | 启动链路用 rust-osdev **bootloader + x86_64 + acpi** 的类型建模（IdtEntry/Gdt/PageTable/VirtAddr） | blog_os / rust-osdev | §1.3/1.4 |
| 16 | 内存预算可测：子系统 **cell 化 + used/limit 台账**（借鉴 Theseus cell 边界、Hubris 确定性内存） | Theseus / Hubris | §5.3、§10.3 |
| 17 | unsafe 占比目标 <5%：RustyHermit 实测 ≈3.3% 佐证可行性 | RustyHermit（PLOS'19） | §6.3⑥ |

### 14.2 各子系统借鉴落地明细

#### ① 启动 / 中断 / 页表（§1.3、§1.4）→ blog_os、rust-osdev

- **采纳**：`IdtEntry/Gdt/PageTable/VirtAddr` 等类型直接按 x86_64 crate 建模；`BootInfo`（内存映射/帧缓冲）作为 bootloader 传递给内核的启动信息结构，对应 §1.3 Phase 1 的 multiboot2 信息。
- **避坑**：若走 GRUB/multiboot2，只借鉴类型不引入 bootloader crate。

#### ② 物理内存（§3.1、§4.1）→ buddy_system_allocator、buddy-slab-allocator、Redox mm

- **采纳**：buddy 分裂/合并的边界实现（`buddy_addr ^ (1 << order) << PAGE_SHIFT`）与测试；slab 对象位图（free 链表用 `Bitmap` 而非 `Vec<*mut u8>`，省 8B/对象）；`LockedHeapWithRescue` 的"耗尽回调"接入 §8.1 的 OOM 分级。
- **避坑**：单核 UP 阶段不引入 per-CPU slab（DESIGN §11 已定），smoltcp/多核相关设计留到 M9+。

#### ③ 虚拟内存（§3.2）→ Theseus

- **采纳**：`MappedPages { pages, frames, flags }` 类型化映射 —— `mmap` 返回该句柄，保证 VA↔PA 双射、访问权限（`MappedPagesMut/Exec`）由类型区分；页表懒分配逻辑对照 rCore。
- **避坑**：Theseus 单地址空间架构与容器隔离冲突，只借类型建模。

#### ④ 调度（§4.2）→ rCore、intrusive-collections、RustyHermit

- **采纳**：CFS runqueue = 侵入式 RBTree（`KeyAdapter` 提取 `vruntime`），哨兵 nil 节点免空指针分支，`O(1)` 取最左 = 选中任务；上下文切换汇编对照 rCore。
- **备选**：若未来引入优先级，参照 RustyHermit 的 `u64` 优先级位图（`leading_zeros` O(1) 选队）。

#### ⑤ 网络栈（§3.8、§4.5）→ smoltcp

- **采纳**：`Socket.rx_buf/tx_buf` 用 RingBuffer（smoltcp `managed::RingBuffer`）；缓冲预算编译期常量（对应 sk_buff 水位）；事件驱动 poll 结构对应 softirq 下半部；TCP 状态机单测组织方式。
- **避坑**：smoltcp 无 SACK/时间戳/select 全语义，是**基线**不是完整栈；TCP 重传/拥塞（Cubic/NewReno）按 §4.5 自实现。

#### ⑥ VFS / 文件系统（§3.6、§13.2）→ ArceOS axfs_vfs、Redox redoxfs

- **采纳**：`VfsOps/VfsNodeOps` 的"必选方法 + 可选方法默认 ENOSYS"trait 模式（§13.2 已内化）；redoxfs 的"元数据统一为带版本键值对 + WAL"思路用于 ext4 有序写（TASKS M10-06）。
- **避坑**：redoxfs 的 B+树/日志结构不照搬（tmpfs 用匿名页即可）。

#### ⑦ 容器 / cgroup / OCI（§3.5、§4.6）→ youki、oci-spec-rs

- **采纳**：libcontainer 的生命周期状态机与 builder 模式（spec → 容器状态）；libcgroups v2 的 memory/pids/cpu 语义（内核侧 reimplement，行为对齐）；oci-spec-rs 的 `config.json` 字段模型作为 OCI 解析清单（TASKS M14-04）。
- **避坑**：youki 是用户态运行时，依赖 syscall；Novos 是内核实现 —— 只借语义与状态机。

#### ⑧ 安全（§13.5）→ seccompiler

- **采纳**：seccomp 的 `filter_action/default_action` 分离 + 参数级匹配语义；按线程/进程类别分别加载 filter 的思路（Novos 按容器粒度）。
- **避坑**：seccompiler 是 BPF 编译器，Novos 需要的是解释器（<500 行），读其指令生成逻辑理解语义即可。

#### ⑨ 驱动 / 设备（§13.3、§13.4）→ virtio-drivers、Tock

- **采纳**：virtio `Hal` trait（DMA 分配/虚实转换）解耦驱动与页分配器；split VirtQueue 三环（desc/avail/used + free_head）；Tock 的 DMA 缓冲静态化原则 + `set_client` 破环模式。
- **避坑**：需把 DMA 层接到 Novos 页分配器 + cgroup `charge/uncharge`，直接移植会绕过记账。

#### ⑩ ext4（§13.3）→ ext4-view-rs、am-fs-ext4、mkext4

- **采纳**：ext4-view-rs 的超级块/inode/dir/extent/htree 解析结构（只读路径直接对照）；am-fs-ext4 的写路径 + JBD2 日志语义；mkext4 生成测试磁盘镜像（TASKS M10-10）。

#### ⑪ 动态链接 / TLS / futex（§13.6–13.8）→ arceos-runlinuxapp、Rust std futex

- **采纳**：ELF 动态段加载 + auxv 栈帧初始化 + `ARCH_SET_FS` 的完整链路（arceos-runlinuxapp 的 `loader.rs/main.rs/task.rs` 直接对照）；futex 按物理页哈希索引等待队列。
- **避坑**：不引入 async runtime（zCore 的异步内核内存开销大，与 32MB 预算相悖）。

#### ⑫ 定时器 / 事件（§4.5、§13.11）→ tokio-time

- **采纳**：第一版最小堆；若 tick 密集、O(n) 触顶，评估 6 层分层时间轮（O(1) 入队/出队，粒度分级）。
- **避坑**：tokio 的时间轮面向用户态异步，内核接入需包一层 `Timer` 抽象。

---

## 15. 工具链支持与 ABI 兼容面（自编译 Go / Rust / C++）

> 核心决策（对应"轻量容器宿主"路线）：**兼容面 = musl 的 syscall 足迹**，而不是"实现全部 Linux syscall"。
> 因为 Novos 对齐 Linux syscall ABI（§1.2），现成工具链 target 直接复用，**无需自定义语言 target**。

### 15.1 三语言的接入方式（零语言后端成本）

| 语言 | 构建方式 | 说明 |
|---|---|---|
| **Go** | `GOOS=linux GOARCH=amd64 CGO_ENABLED=0` | 纯静态二进制，无自定义 target |
| **Rust** | `x86_64-unknown-linux-musl` target（rustup 现成） | `cargo build --target ... --release` 直接出静态 musl 二进制 |
| **C++** | musl-cross（musl.cc 预编译或 musl-cross-make）+ `-static -static-libstdc++ -static-libgcc` | 复用现成工具链 |

> 三者本就为 Linux ABI 生成代码，Novos 兼容的就是这个 ABI —— 语言层零改造。

### 15.2 新增工作量（约 2–3 人月，单人）

| 工作项 | 内容 | 里程碑归属 |
|---|---|---|
| 交叉工具链搭建 | musl-cross + crt1/crti/crtn + linker script + 版本锁定 | M11 |
| musl 适配 | 跑通 musl；把"musl 需要的 syscall"定成兼容面清单 | M11 |
| ABI 契约文档化 | syscall 清单、结构体布局、errno、调用约定 → SDK 文档 | M11（持续维护） |
| 测试框架 + CI | 编译 → 打包 → QEMU 真跑 + 断言 + 示例程序 | M11/M14 |
| 语言各自增量 | Go 1–2 周、Rust 1–3 周、C++ 2–4 周（各自怪癖） | M14 |

### 15.3 兼容面收敛原则

- **以 musl 的 syscall 足迹为边界**：只保证"musl + 目标程序"用到的路径全对，其余 syscall 返回 `ENOSYS`；
- **可测试、可回归**：每个新增 syscall 有对应 host 测试 + QEMU 集成断言；
- **与 §13.6 动态链接的关系**：容器服务默认静态编译（Go/Rust/C++）；musl 动态链接（ld-musl）为 musl 生态二进制服务，两者并存；
- **SDK 文档为交付物**：`docs/abi.md` 维护 syscall 清单/结构体/errno/调用约定，作为工具链适配的契约。

---

## 16. 容器形态：OCI 镜像 + 轻量运行时 + OTA（"Docker" 重述）

> 定位收敛后，"支持 Docker" **重述为**：**支持 OCI 镜像格式（pull/解压/摘要校验）+ 轻量容器运行时（namespace/cgroup/overlayfs 之上）+ 镜像分发（OTA 升级回滚）**——**不做 docker daemon / CLI 兼容**。
> 理由：Novos 只跑"为 Novos 编译"的 musl 静态子集镜像，"任意镜像 pull 下来就能跑" 的 Docker 核心卖点不成立；完整 dockerd/containerd 是负担不是价值。

### 16.1 价值排序（嵌入式核心需求）

| 用处 | 说明 | 定位 |
|---|---|---|
| **OTA 升级 + 回滚（最大价值）** | OCI 镜像分层 + SHA-256 摘要 + 不可变 → 设备只增量拉取变化的层，出问题切回旧层 | 嵌入式设备第一需求，镜像格式 = 现成 OTA 载体 |
| **多服务隔离 + 资源限额** | namespace/cgroup：网关、Redis、业务逻辑互相隔离，内存/CPU 可限额 | 多服务网关基础 |
| **标准部署包** | "为 Novos 编译的程序 + 依赖 + 配置" 打包成镜像 = 统一交付物 | 替代手工拷二进制 |
| **快速恢复** | 容器无状态可重建：坏了删掉重拉 | 设备远程维护关键 |
| **安全隔离** | 管理面/数据面分离、服务间攻击面隔离 | 安全卖点支撑 |
| **开发/生产一致性** | 开发环境跑同一份镜像 = 目标设备环境 | 降低开发成本 |

### 16.2 明确砍掉的部分

- **docker daemon + docker CLI 兼容**：不兼容 CLI 语法/API/daemon 架构；
- **Docker Hub 完整生态依赖**：不追求"能跑别人的任何镜像"。

### 16.3 工程含义（工作量减一个数量级）

只需：一个轻量 **`novos-pull`**（走 registry HTTPS + OCI 解析 + 摘要校验）+ 轻量容器运行时（§4.6 流程）+ 镜像打包工具（OCI layer 构建，配合 §15 自编译工具链）。OTA 升级/隔离/部署核心价值一个不少。

---

## 17. ARM64 / RISC-V 演进（终局目标）

> x86_64 起步（QEMU 开发便利）；**ARM64 为终局目标，RISC-V 留口**——arch 层隔离从第一天做好（M0 已按此组织：`boot.asm` 独立于内核逻辑，页表/中断/上下文切换收敛在 arch 边界）。
> 嵌入式生态 ARM/RISC-V 并存，`arch/` 目录设计时把 `aarch64` 与 `riscv64` 都留口（§19.1 架构骨架）。

### 17.1 arch 抽象边界（x86_64/arm64 各自实现）

| 边界 | x86_64（M0 已有雏形） | arm64 | riscv64（留口） |
|---|---|---|---|
| 启动 | boot.asm（multiboot2 + 长模式） | boot.S（u-boot/QEMU virt + `_start`） | boot.S（OpenSBI + `_start`） |
| 页表 | 4 级页表（PML4→PT） | 4 级页表（TTBR0/TTBR1，PGD→PTE） | Sv39/Sv48 |
| 中断 | IDT + PIC/APIC | VBAR + GIC | mtvec/stvec + PLIC |
| 上下文切换 | 通用寄存器保存/恢复 | 同构（X0-X30 + SP/ELR） | 同构（X1-X31 + SP） |
| 原子/屏障 | `asm!` 内联 | `asm!` 内联（dmb/isb） | `asm!` 内联（fence） |
| 虚拟化 | 无（第一版） | 无（第一版） | 无（第一版） |

### 17.2 移植顺序与预算

- M9 之后正式评估（里程碑 M15）：QEMU `virt` 平台起 aarch64 原型（串口 + 内存映射 + 调度 + 容器冒烟）；
- 迁移原则：**内核逻辑与 arch 解耦**（`mm/sched/net/fs` 不感知 arch），仅 `arch/` 与驱动层有平台差异；
- 预算：ARM64/RISC-V 指令密度与 x86_64 相当，32MB 预算口径不变；
- 目标设备：ARM 网关盒子（256MB–2GB），x86_64 定位为开发/演示环境。

---

## 18. 用户态生态矩阵与并发结论（语言 / 组件 / 消息）

### 18.1 并发结论：线程是硬需求，协程是免费附赠

- **多线程是硬需求**：Go goroutine、Rust `std::thread`、C++ 线程、Redis 多线程全部建立在 OS 线程之上——兼容层必须把 **`clone + futex + TLS`** 做对（§13.7/§13.8、M2/M11 地基项）；
- **协程不需要内核专门支持**：Go/Rust async/Python asyncio 协程全在用户态实现，内核只需 **epoll + 非阻塞 IO + 时钟**（§3.8、M5）；
- **结论**：把线程 + epoll 做对，多线程与协程同时获得；**内核不为协程多付任何工作**。

### 18.2 语言矩阵

| 等级 | 语言 | 说明 |
|---|---|---|
| 🟢 必支持 | C / Rust / Go（musl 静态） | 现成 target 复用（§15），嵌入式主力 |
| 🟢 值得 | Lua（极轻脚本）、MicroPython、QuickJS（轻量 JS） | 各"轻"版本均可行 |
| 🟡 可选 / 远期 | CPython、JVM | 需要时评估（musl 构建风险高） |
| ❌ 不推荐 | Node、Erlang/Elixir、.NET | 与轻量定位相反 |

### 18.3 组件矩阵

| 等级 | 组件 |
|---|---|
| 🟢 必支持 | Redis（缓存 + 消息）、SQLite（数据）、轻量 HTTP 服务（网关）、busybox、musl 交叉工具链 |
| 🟢 值得 | Mosquitto（MQTT）、Lua / MicroPython / QuickJS |
| 🟡 可选 | NanoMQ、ZeroMQ、CPython |
| ❌ 排除 | ActiveMQ、RabbitMQ、Kafka、MySQL、PostgreSQL、Node、Erlang |

### 18.4 消息队列路线（每类需求用最轻的那个）

| 需求 | 方案 | 说明 |
|---|---|---|
| 设备内消息 / 队列 / 发布订阅 | **Redis Streams / Pub-Sub**（Redis 自带） | 零新增组件，M14 Redis 任务覆盖 |
| IoT 设备接入（MQTT） | **Mosquitto**（C 轻量 broker，musl 静态） | M14 值得项 |
| 进程间轻量消息 | ZeroMQ（libzmq） | 可选 |
| ActiveMQ / RabbitMQ / Kafka | 排除 | Java/Erlang 生态，重且无必要 |

> 定位原则：**每个需求只选最轻的方案**；服务间/设备间通信优先 Redis/MQTT，不引入重量级消息中间件。

### 18.5 组件评估补充（价值闭环导向）

> **边缘网关价值闭环**（组件只围绕这条链选）：**Modbus 采集 → JSON → MQTT/HTTP 上报 → Web 界面监控**。

| 等级 | 组件 | 说明 |
|---|---|---|
| 🟢 必支持（零难度） | JSON / CSV | 语言库自带；JSON 是现代设备全场景必须 |
| 🟢 值得（功能重要、难度稍大） | **Modbus 工业协议**（网关核心）、**内置 Web 管理界面**（轻量 HTTP + 静态前端）、MQTT 客户端、轻量 TLS（mbedTLS/rustls） | "美观" 的正确解法 = Web 管理界面，设备管理标配 |
| 🟡 远期 | WebSocket、NTP、OPC-UA、边缘本地推理 | 按需再评估 |
| ❌ 不建议 | Excel/docx、PHP | 设备不处理文档格式；PHP 不是"前端美观"的答案 |

### 18.6 AI 调用评估（并入 HTTP 用例）

- **云端大模型 API = HTTP + TLS + JSON 一个用例**，不需要单独支持；
- HTTP 客户端（§5 网络栈之上）补三条能力：**SSE 流式响应、长超时、大 JSON 流式解析**（避免一次缓冲）；
- **本地推理**是另一码事（推理引擎 + 数学库 + NPU 驱动），维持远期独立子系统结论。

---

## 19. 架构骨架与演进分级（架构级 / 功能级 / 产品级）

> **最重要的设计约束**：下面"架构级"四项不是"以后加的功能"，而是**决定第一天代码怎么分层的约束**——第一版不预留，后面会推倒重来。

### 19.1 架构级（第一版必须预留，否则返工成本极高）

| 维度 | 具体内容 | 为什么必须提前 | 落地位置 |
|---|---|---|---|
| **设备驱动模型** | **bus→device→driver 统一框架** + 板级包（BSP）+ 中断分发；覆盖 GPIO/I2C/SPI/**CAN**/多路 UART/PWM/ADC | 现在只有 virtio/uart/timer 三个驱动；驱动模型不在第一版定型，每加一个外设推一次架构 | §1.1、§6.2⑤、§13.4 |
| **实时性 / 确定性** | 中断优先级、可抢占内核、**RT 调度类（优先级 + 抢占）+ 普通类（CFS）双队列** | 工业/车用要求确定性延迟，纯 CFS 不够；不预留则加实时性 = 重构调度器 | §4.2、§3.3 `SchedEntity` |
| **时钟 / 中断框架** | 通用时钟源抽象、高精度 timer、**RTC**、**monotonic** 时钟 | 嵌入式拿到的是具体硬件（不同 timer/RTC）；很多用户态程序依赖 monotonic | §6.2⑥、§1.4 |
| **快速启动预留** | 秒级冷启动：只初始化必需驱动、**deferred init（延迟初始化）**、直接映射优化 | 冷启动快是嵌入式核心卖点；启动路径（什么先起、什么懒加载）第一版就要定 | §1.3 启动流程 |
| **RISC-V 预留** | `arch/` 目录把 `aarch64` 与 `riscv64` 都留口 | 嵌入式 ARM/RISC-V 并存，分层越早越省 | §17 |

> **驱动跟着锁定的目标设备走，不预先全做**（2026-08 决策）：

| 驱动 | 首期 | 中期 | 决策依据 |
|---|---|---|---|
| UART / 定时器 / virtio | ✅ | — | 开发基础 |
| **USB Host 最小集**（串口 / U 盘 / 网卡） | ❌ | ✅ | 网关场景实用 |
| Type-C 完整协议（PD/alt-mode） | ❌ | ❌ | 无需求，只当供电通道 |
| 音频（最小 PCM 输出） | ❌ | 仅设备需要时 | 默认无需求 |

> 下一步：**定下第一块真实目标板**（如某块 ARM 工业板），按它的外设清单定驱动清单，而不是在真机上发车前把驱动做齐。

### 19.2 功能级（中期补，不影响架构，但要进路线图）

| 项 | 内容 |
|---|---|
| 电源管理 | idle 指令级低功耗（x86 hlt / ARM WFI-WFE）、外设电源门控、suspend/resume、调频 —— 电池/太阳能设备第一需求 |
| Flash 文件系统 | 设备实际用 flash（非磁盘）：littlefs/ubifs（磨损均衡 + 掉电安全）；ramfs/tmpfs 只是"内存与开发"，存储层单独设计 |
| 看门狗 | 硬件 watchdog + 软件喂狗，防死机自愈 —— 嵌入式标配 |
| 掉电保护 | 日志原子写、文件系统一致性（配合 SQLite WAL）、电源异常恢复 |
| 可观测性 | 环形日志 + 落盘 + 远程日志、健康指标（内存/fd/CPU）、配置下发通道 |
| GDB 调试生态 | 内核态 + 用户态调试、panic 可读化、死机转储（crash dump 供远程诊断） |

### 19.3 产品级（商业化才需要，个人项目可暂缓）

| 项 | 内容 |
|---|---|
| 安全启动 | Secure Boot + 内核签名 + 信任根 —— 做产品是准入项 |
| 完整 OTA 链路 | 下载 → 签名校验 → **A/B 分区切换** → 失败回滚（§16 只是 OCI 镜像雏形） |
| 标准合规 | IEC 62443（工业）、ISO 26262（车用）——若目标行业是这两块，提前了解 |
| 远程运维协议 | 轻量管理通道（非 SSH 那种重的） |

---

## 20. 交互模式（无头设备的三层通道）

> 来源：`interaction.md`。设备无头（无屏幕键盘），一切交互在电脑端远程操作；开发期 QEMU = 生产期真机，交互心智完全一致。

### 20.1 三层交互通道

| 层级 | 通道 | 电脑端工具 | 场景 | 是否连线 |
|---|---|---|---|---|
| 用户层 | **Web 管理界面** | 浏览器 `http://<设备IP>` | 日常管理、拉取/运行容器、看状态 | 网络远程 |
| 开发层 | **SSH / 串口 Console** | dropbear / PuTTY + USB 转串口 | 开发调试、救援 | SSH 远程；串口物理连线 |
| 运维层 | **Agent 主动上联** | 云管理平台网页 | 规模化部署、远程 OTA | 设备主动连平台 |

### 20.2 镜像拉取流程（"docker pull" → `novos-pull`）

- **手动**：Web 界面点"拉取镜像" → 设备端 `novos-pull`（§16.3）连 registry（HTTPS + token 认证）→ SHA-256 校验 → 解压 → 本地镜像仓库 → 点"运行" → 界面显示状态；
- **自动**：设备上电 → Agent 上联云平台 → 下发"部署 xx 版本" → 自动 pull/校验/运行 → 回报状态/日志 → 异常一键回滚（OCI 层复用，只传变化部分）；
- **离线**：设备连不上公网时，可上网电脑 `docker save` 导出 tar → Web 上传或 U 盘拷入（需兼容 Docker/OCI archive 格式）。

### 20.3 设计落点

| 通道/能力 | 内核/用户态落点 | 里程碑 |
|---|---|---|
| Web 管理界面 | 轻量 HTTP 服务 + 静态前端打包进 rootfs（§5 网络栈之上） | M14 |
| SSH | dropbear（musl 静态，§15 工具链）+ devpts/PTY（§13.4，M12 已有） | M14 |
| Agent 上联 | 用户态自编译程序（§15），走 HTTPS + JSON（§18.6） | M14 |
| 离线导入 | OCI/Docker archive 解析（复用 §16.3 `novos-pull` 的解压/校验链） | M14 |

---

*本文档为规划稿，随实现推进持续修订。每个里程碑落地后回填实测内存数据。*
