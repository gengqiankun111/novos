# Novos-OS 参考组件调研（GitHub Rust 开源项目）

> 调研时间：2026-08-25。本文档把 GitHub 上可参考的 Rust 开源组件，按 **Novos-OS 的子系统/里程碑** 做了映射，给出每个组件可以借鉴的 **数据结构、算法、设计思路**，以及 **避坑点**。
>
> 使用方式：**借鉴思路与接口设计，不直接引入依赖**。内核必须自包含（`no_std`），且 32MB/40MB 预算内不允许背一个大型外部 crate。所有引用均指向各项目源码目录，实施时以目标仓库 `main/master` 分支为准。

---

## 1. 总览表（按子系统映射）

| # | 参考组件 | 仓库 | 一句话定位 | 对应 Novos 子系统 / 里程碑 | 主要可借鉴点 |
|---|---|---|---|---|---|
| 1 | Redox OS | github.com/redox-os/kernel | 全 Rust 微内核 + scheme 架构 | 整体架构、驱动、FS（M0–M14） | 驱动用户态化思路、ralloc/redoxfs/relibc、buddy 分配 |
| 2 | Theseus | github.com/theseus-os/Theseus | 语言内设计（intralingual）OS | 内存管理、调度（M1/M2） | 类型化内存映射（MappedPages）、cell 边界、编译期不变量 |
| 3 | Tock | github.com/tock/tock | 嵌入式 Rust OS，capsule/grant | 生命周期、DMA 缓冲（M0/M1/M12） | 静态/栈/grant 三类分配、DMA 缓冲静态化、set_client 破环 |
| 4 | ArceOS | github.com/arceos-org/arceos | 清华组件化内核（ax\* crate 家族） | 全流程模块划分、feature 裁剪 | axmm/axtask/axfs/axdriver 模块切分、axfeat 特性开关 |
| 5 | rCore | github.com/rcore-os/rCore | 类 Linux 单体内核（教学） | M0–M9 全部基础子系统 | 页表/任务/调度/syscall/驱动 全链路最小实现 |
| 6 | zCore | github.com/rcore-os/zCore | Zircon 重写 + Linux 兼容 | syscall 模块化（M3/M13） | linux-syscall 分模块、LibOS 用户态运行调试 |
| 7 | RustyHermit | github.com/hermit-os/kernel | Rust unikernel | 调度、异步任务（M2/M9） | 优先级位图 O(1) 调度、unsafe ≈3.3% 佐证预算可行 |
| 8 | Hubris | github.com/oxidecomputer/hubris | Oxide 的确定性 RTOS | 任务/内存确定性（M2） | 静态任务集、零动态分配、syscall ABI 设计 |
| 9 | blog_os + rust-osdev | github.com/phil-opp/blog_os | x86-64 Rust OS 教程 + 配套 crate | M0 引导/中断/页表 | bootloader/x86_64/acpi crate、QEMU 测试框架 |
| 10 | buddy_system_allocator | github.com/rcore-os/buddy_system_allocator | buddy 内核分配器 | M1 物理内存 | FrameAllocator + LockedHeapWithRescue（OOM 回调） |
| 11 | buddy-alloc | github.com/jjyr/buddy-alloc | no-MMU buddy + 链表快路径 | M1 物理内存 | 快分配器 + buddy 组合、无 syscall 设计 |
| 12 | buddy-slab-allocator / slabmalloc | github.com/weclaw1/buddy-slab-allocator | SLUB 风格 slab | M1 内核堆 | bitmap 对象位图、跨 CPU 释放、size class |
| 13 | smalloc | github.com/zooko/smalloc | 406 行极简分配器 | M1（学习参照） | 极简可读、按 size class 分 slab |
| 14 | ralloc | github.com/redox-os/ralloc | Redox 用户态分配器 | M1（学习参照） | thread-local 免锁分配思路 |
| 15 | intrusive-collections | github.com/Amanieu/intrusive-rs | 侵入式链表 + 红黑树 | M2/M4/M5 全栈容器 | 内核风格侵入式链表/RBTree、Cursor 安全遍历 |
| 16 | embed-collections | github.com/ydrmaster/embed-collections | cache-aware B+树 / 侵入式 AVL | M4 目录、M2 调度 | B+树目录（来自 ZFS 的 AVL）、SegList |
| 17 | smoltcp | github.com/smoltcp-rs/smoltcp | no_std 无堆 TCP/IP 栈 | M5 网络栈 | 事件驱动 poll 模型、RingBuffer、编译期缓冲预算 |
| 18 | youki | github.com/youki-dev/youki | Rust OCI 容器运行时 | M8/M14 容器 | libcontainer 生命周期、libcgroups v2、seccomp/cap 应用 |
| 19 | oci-spec-rs | github.com/youki-dev/oci-spec-rs | OCI 规范 Rust 类型 | M14-04 OCI 解析 | config.json 类型模型（直接映射内核创建参数） |
| 20 | seccompiler | github.com/firecracker-microvm/firecracker | seccomp BPF 编译器 | M12-10 Seccomp | JSON 规则 → BPF 字节码、规则匹配语义 |
| 21 | virtio-drivers | github.com/rcore-os/virtio-drivers | VirtIO guest 驱动 | M0/M5/M10 | Hal trait（DMA/虚实转换）、split VirtQueue |
| 22 | ext4 三件套 | ext4-view-rs / rust-fs-ext4 / mkext4 | ext4 只读 / 读写 / mkfs | M10 ext4 | 无堆 ext4 解析、JBD2 日志、确定性 mkfs（测试用） |
| 23 | arceos-runlinuxapp | github.com/rcore-os/arceos-runlinuxapp | 在 ArceOS 上跑 musl ELF | M11 动态链接 | ELF 加载、auxv 初始化、ARCH_SET_FS/TLS |
| 24 | getrandom | github.com/rust-random/getrandom | 无堆随机源 | M12 熵源 | RDRAND 封装、熵池混合 |
| 25 | tokio-time / std futex | github.com/tokio-rs/tokio | 分层时间轮 / futex 语义 | M2 定时器、M11 futex | 6 层分层时间轮、futex WAIT/WAKE 表 |

---

## 2. 完整内核级项目

### 2.1 Redox OS —— 全 Rust 微内核（架构参照）

- 仓库：<https://github.com/redox-os/kernel>
- 定位：完成度最高的全 Rust OS，微内核 + scheme（类 Plan9 文件式 IPC）模型，驱动/文件系统/网络全在用户态 daemon。

**可借鉴（对应 Novos 模块）**
- **buddy 物理内存**：Redox kernel 用 buddy 管理物理帧（`mm` 目录），与 DESIGN.md §3.1 的 `BuddyAllocator` 结构一致，可对照其分裂/合并边界条件。
- **ralloc（Redox 的默认分配器）**：thread-local 免锁分配模型 —— Novos 单核第一版可用全局 Spinlock 简化，但 ralloc 的 size-class + 空闲链结构值得读（`redox-os/ralloc`）。
- **redoxfs（TFS）**：日志结构 + B+树 元数据模型 —— Novos 的 tmpfs/ext4 不需要照搬，但"元数据统一建模为带版本键值对 + WAL"的**崩溃一致性思路**可借鉴到 ext4 data=journal 写路径（TASKS M10-06）。
- **relibc / 驱动分离**：证明"Rust libc 替代 musl"可行，但 Novos 目标明确用 musl，不引入。

**避坑**
- scheme/IPC 是微内核模型，与 Novos 单体内核定位冲突 —— **只借鉴组件内部数据结构，不借鉴进程间架构**。
- Redox 用户态驱动通过 `iopl`/`/scheme/memory/physical` 访问硬件，Novos 驱动在内核态直接 MMIO，无此中间层。

### 2.2 Theseus —— 语言内设计（类型安全内存建模）

- 仓库：<https://github.com/theseus-os/Theseus>
- 定位：从零用 Rust 写的研究型 OS，单地址空间（SAS/SPL），把资源管理"下沉"到编译器。

**可借鉴（对应 Novos 模块）**
- **类型化内存映射（核心亮点）**：`AllocatedPages` / `AllocatedFrames` / `MappedPages` 三组类型，用 Rust 所有权保证 **VA↔PA 映射是双射**，并用 `MappedPagesMut` / `MappedPagesExec` 区分读写/执行权限（`kernel/memory`）。
  - → 对应 Novos DESIGN §3.2 的 `Mm/Vma`：可以让 `mmap` 返回 `MappedPages` 这类 handle 而不是裸 `PhysAddr`，从类型层面杜绝"映射了但忘了记录/访问权限错配"。
- **cell 边界 + 编译期不变量**：所有组件以"cell"（对应 object 文件）为运行时边界，带依赖元数据 —— 对应 Novos"每子系统内存预算可独立审计"的工程目标（DESIGN §5.3）。
- **调度器**：Theseus 的 `kernel/scheduler` 用语言特性管理任务状态机，比 Linux `task_struct.state` 位运算更可读。

**避坑**
- Theseus 是**单地址空间、单特权级**，没有用户态/内核态隔离 —— 与 Novos 容器隔离目标（DESIGN §9.2）**根本冲突**。只借"intralingual 思路"，不借架构。
- 它没有 namespace/cgroup 概念，这些子系统无参考价值。

### 2.3 Tock —— 生命周期与 DMA 缓冲管理

- 仓库：<https://github.com/tock/tock>
- 定位：面向 MCU 的嵌入式 Rust OS，内核=调度+内存保护+IPC，驱动以 Rust "capsule" 编译进内核。

**可借鉴（对应 Novos 模块）**
- **三类分配生命周期模型**（`doc/Lifetimes.md`）：静态（`'static`）/ 栈（词法生命周期）/ grant（运行时生命周期）。
  - → 精确对应 Novos DESIGN §1.3 **Phase 1 无堆** 约束：启动早期只能静态分配，Tock 对"哪些对象必须 `'static`"的分类方法可以直接抄。
- **DMA 缓冲静态化原则**：传给硬件的异步缓冲必须静态分配，保证硬件持有期间不被回收（`send_buffer(&'static [u8])` 签名）。
  - → 对应 Novos virtio-net/blk 的 `Skb`/`Bio` 缓冲管理（DESIGN §3.8、TASKS M10-01）。
- **set_client 破环模式**：capsule 间循环依赖通过 `set_client()` 后期注入解决 —— 对应 Novos `Arc<dyn>` 驱动的注册模式。
- **syscall/驱动 trait 分层**：`kernel/src/hil/` 定义定时器/UART/GPIO 标准接口，芯片实现与内核逻辑解耦 —— 对应 Novos `CharDevice`/`NetDevOps` trait 设计（DESIGN §13.4）。

**避坑**
- Tock 无虚拟内存、进程集静态 —— 容器场景不适用，只借缓冲/生命周期管理。

### 2.4 ArceOS —— 组件化内核（模块切分样板）

- 仓库：<https://github.com/arceos-org/arceos>
- 定位：清华系实验性组件化 OS（unikernel 形态），把内核拆成 50+ 个独立 `ax*` crate。

**可借鉴（对应 Novos 模块）**
- **按子系统拆 crate**：`axalloc`（分配器）、`axmm`（虚拟内存）、`axtask`（任务/CFS）、`axsync`（锁）、`axfs`（VFS/ramfs/devfs）、`axdriver`（virtio/pci）、`axhal`（硬件抽象）、`axruntime`（启动）、`axconfig`（编译期配置）—— 与 Novos DESIGN §1.1 分层**几乎一一对应**，可直接作为 Novos 的 crate 划分参照。
- **axfeat 特性开关**：用 Cargo feature 控制组件组合 → 对应 Novos DESIGN §13.15 的 `minimal`/`full` feature 策略，可对照其 `Cargo.toml` 组织方式。
- **axmm + page_table_multiarch**：`page_table_multiarch` crate 抽象 4 级页表，`axmm` 做用户地址空间 —— 对应 Novos §3.2 页表懒分配。
- **axfs_vfs trait**：`VfsOps/VfsNodeOps` 抽象与 Novos DESIGN §13.2 的 `FileSystemDriver` trait 是同一思路（可对照其可选操作默认 `ENOSYS` 的写法）。
- **axdriver_block + axdriver_virtio**：virtio-blk 与块设备 trait 分离 —— 对应 Novos `BlockDevice` trait（DESIGN §13.3）。

**避坑**
- ArceOS 是 unikernel（应用与内核同地址空间），无进程隔离；它部分组件（如 smoltcp 集成）依赖 std 生态，Novos 需 `no_std` 重写。

### 2.5 rCore —— 类 Linux 单体内核（全链路最小实现）

- 仓库：<https://github.com/rcore-os/rCore>
- 定位：清华教学用类 Linux Rust 内核，从启动到用户态 shell，支持 musl 二进制与多架构。

**可借鉴（对应 Novos 模块）**
- **M0**：启动、串口、中断、页表初始化（对照 blog_os）。
- **M1**：`FrameAllocator` + `HeapAllocator`，物理页管理实现与 DESIGN §3.1 结构一致。
- **M2**：页表（懒分配、COW）、任务/上下文切换汇编、CFS vruntime 红黑树 —— 与 DESIGN §4.2 伪代码几乎逐行对应，是 **Novos 调度器最直接的参考**。
- **M3**：syscall 表 + 参数解析 + ELF 加载 + 简易 shell。
- **M4**：VFS（Inode/Dentry/File）+ 简单 FS。
- **M5（rCore 补充章节）**：virtio-blk 驱动 + 中断 I/O，`Hal` trait 抽象 —— 与 DESIGN §13.3 BIO 设计同构。
- **测试**：rCore-Tutorial-Book 提供 QEMU 集成测试组织方式，可对照 Novos §12 测试架构。

**避坑**
- rCore 没有 namespace/cgroup/OverlayFS/完整 TCP（教学内核砍掉了容器与网络完整性），这些子系统要参考 youki/smoltcp。
- 它的并发模型是单核简化版，与 Novos UP 第一版一致，可直接搬；SMP 部分无参考价值。

### 2.6 zCore —— 模块化 syscall + LibOS 调试模式

- 仓库：<https://github.com/rcore-os/zCore>
- 定位：用 Rust 重写 Zircon 微内核并提供 Linux 兼容层，可作 LibOS 在用户态运行。

**可借鉴（对应 Novos 模块）**
- **linux-syscall / linux-object 分 crate**：syscall 处理按对象（File/Process/Thread/Signal）分模块 —— 对应 Novos 的 syscall 层组织（DESIGN §1.2）。
- **LibOS 模式**：内核作为普通用户进程运行，可用 gdb/cargo test 调试 —— **强烈建议 Novos 给逻辑模块（buddy/slab/rbtree/TCP 状态机）保留 host 可测接口**（DESIGN §12.2 已要求）。
- **异步内核**（futex 等待队列用 async 实现）：可对照 Novos `WaitQueue`/futex 设计（DESIGN §13.7），但 Novos 第一版用同步队列更省内存。

**避坑**
- zCore 近年更新放缓（2022 后基本停滞），只作思路参考，不作依赖。
- 异步内核内存开销大，与 Novos 32MB 预算取向相反 —— **不引入 async runtime**。

### 2.7 RustyHermit —— unikernel（调度位图 + unsafe 佐证）

- 仓库：<https://github.com/hermit-os/kernel>（hermit-rs 应用侧）
- 定位：Rust unikernel，面向高性能云。

**可借鉴（对应 Novos 模块）**
- **优先级位图 O(1) 调度**：`prio_bitmap: CachePadded<u64>` + `leading_zeros` 找最高优先级队列 —— 一个 u64 即可 O(1) 选择队列，比红黑树更省内存；可作为 Novos 调度器（DESIGN §4.2）的补充方案（如果 Novos 未来引入优先级）。
- **异步任务执行器**：内核内嵌轻量 async 任务 —— 对应 Novos softirq 下半部（DESIGN §1.4）可参考其唤醒机制，但同样**不引入完整 async runtime**。
- **unsafe 占比实测 ~3.27%**（PLOS'19 论文数据）：与 Novos `<5%` 目标一致，**证明预算可行**，可作为 CI 断言上限的参照。

**避坑**
- unikernel 无进程/容器概念，namespace/cgroup 无参考。
- 其锁体系针对 SMP 多核设计，Novos UP 第一版用更简单的 Spinlock。

### 2.8 Hubris —— 确定性嵌入式内核（内存确定性）

- 仓库：<https://github.com/oxidecomputer/hubris>
- 定位：Oxide 的 MCU RTOS，内核 ≈2000 行，静态任务集、零动态分配。

**可借鉴（对应 Novos 模块）**
- **确定性内存模型**：任务集、内存区域全部构建期固定，杜绝运行时分配失败 —— 对应 Novos "每个子系统 used/limit 是硬预算"的工程哲学（DESIGN §5.3），可作为**内核固定组件（idle/init/中断栈）的内存核算方法**。
- **syscall ABI 设计**（`doc/reference` + `syscalls` 定义）：接口稳定、消息有界 —— 对应 Novos syscall 层 errno 对齐策略。
- **Idol IDL**：任务接口用 IDL 生成类型安全代码 —— 对应 Novos 驱动 trait 的接口定义纪律（可参考，不必照搬）。

**避坑**
- 静态任务集与 Novos"动态创建容器"冲突 —— 只用于内核自身固定组件，不用于任务模型。
- Hubris 无虚拟内存/无用户进程，进程管理无参考价值。

### 2.9 blog_os + rust-osdev —— x86-64 引导/中断/页表工具链

- 仓库：<https://github.com/phil-opp/blog_os>；配套：<https://github.com/rust-osdev/bootloader>、<https://github.com/rust-osdev/x86_64>
- 定位：Writing an OS in Rust 系列教程与生产级基础 crate。

**可借鉴（对应 Novos 模块，全部对应 M0）**
- **bootloader crate**（BIOS+UEFI，纯 Rust+内联汇编）：接管实模式→长模式、初始页表、e820 内存映射，`entry_point!` 宏传入 `BootInfo` —— 对应 Novos DESIGN §1.3 Phase 1。
- **x86_64 crate**：`IdtEntry`、`Gdt`、`PageTable`、`VirtAddr` 类型、MSR/port I/O 封装 —— Novos DESIGN §1.4 的 IDT/GDT/页表结构可以直接按此建模（可 `#[cfg(feature)]` 复用或对照重写）。
- **acpi crate**：ACPI 表解析（M9 加 SMP/定时器用）。
- **QEMU 测试框架**：教程提供了在 QEMU 里跑 `cargo test` 的方法 —— 对应 Novos §12.3 集成测试。

**避坑**
- bootloader crate 是"拿来用"的引导器；Novos 若走 GRUB/multiboot2（DESIGN §1.3 写法），则只借鉴 x86_64/acpi 类型，不引入 bootloader。

---

## 3. 内存管理 / 数据结构类

### 3.1 物理内存分配器

| 组件 | 仓库 | 定位 | 借鉴点 | 对应 Novos |
|---|---|---|---|---|
| buddy_system_allocator | rcore-os/buddy_system_allocator | 内核 buddy + 全局堆 | `FrameAllocator`（物理页）、`LockedHeap`、**`LockedHeapWithRescue`（堆耗尽回调 → 触发 shrink/OOM-kill，正是 DESIGN §5.2 预算工程点）** | M1 / §3.1 |
| buddy-alloc | jjyr/buddy-alloc | no-MMU buddy | 链表快分配器 + buddy 组合、无 syscall（静态堆区间） | M1 |
| buddy-slab-allocator | weclaw1/buddy-slab-allocator | SLUB 风格 slab | **bitmap 对象位图**、跨 CPU 免锁释放、size class；与 DESIGN §3.1 `SlabCache` 结构同构 | M1 / §3.1 |
| slabmalloc | Stanford 系 | 高性能 slab | per-size-class slab 池 | M1（对照） |
| smalloc | zooko/smalloc | 406 行极简分配器 | 极简可读，作为"少即是多"的样板 | M1（学习） |
| ralloc | redox-os/ralloc | Redox 分配器 | thread-local 免锁分配 | M1（学习） |

**算法要点**：buddy 分裂/合并边界（`buddy_addr ^ (1 << order) << PAGE_SHIFT`）在 buddy_system_allocator 与 Redox kernel 里有完整测试，可直接对照 Novos DESIGN §4.1。

### 3.2 内核容器与数据结构

| 组件 | 仓库 | 定位 | 借鉴点 | 对应 Novos |
|---|---|---|---|---|
| intrusive-collections | Amanieu/intrusive-rs | 侵入式链表/红黑树 | **侵入式节点 = 对象内嵌 link（同 Linux list_head/rb_node），零分配；RBTree 支持 KeyAdapter + Cursor 安全变更** → 用于调度 runqueue（vruntime）、VMA 树、dcache/sk_buff LRU | M2/M4/M5 全栈 |
| intrusive-red-black-tree | docs.rs（no_std） | 哨兵 nil 节点 RBTree | 哨兵节点免空指针分支、O(1) 缓存最左节点（CFS pick_next 直接取最左） | M2 调度 |
| embed-collections | ydrmaster/embed-collections | cache-aware B+树 + 侵入式 AVL | **B+树目录**（对应 DESIGN §6.2 ①"BTreeMap 排序目录"的进阶版）、来自 ZFS 的侵入式 AVL、SegList（cache 友好小对象列表） | M4 目录、M2 |
| tokio-rs/slab | tokio-rs/slab | 预分配 arena | 稳定句柄（token）访问预分配槽位 → 可作 fd 表 / inode 表 arena 参考 | M3/M4 |

**算法要点**
- 侵入式容器 = Linux 内核风格，**Novos 若想保留"对象常驻、容器零分配"（32MB 关键），应优先采用侵入式 RBTree/链表而非 `BTreeMap<...>`**；`std::collections::BTreeMap` 每次操作都会分配节点，缓存类结构（dcache/sk_buff/runqueue）用侵入式更省。
- 哨兵节点 + 位向旋转（`dir ^ 1`）是红黑树代码减半的经典技巧。

---

## 4. 网络栈类

### 4.1 smoltcp —— no_std 无堆 TCP/IP 栈（M5 首选参考）

- 仓库：<https://github.com/smoltcp-rs/smoltcp>
- 定位：裸机 TCP/IP 栈，事件驱动、显式 poll、**不需要堆**，stable Rust，环回吞吐 Gbps 级。

**可借鉴（对应 Novos M5 / DESIGN §3.8）**
- **无堆 + 编译期缓冲预算**：`IFACE_MAX_ADDR_COUNT` / `FRAGMENTATION_BUFFER_SIZE` / `REASSEMBLY_BUFFER_COUNT` 编译期常量控制缓冲 —— **与 Novos sk_buff 水位/预算控制（DESIGN §2.1-B、§5.2）完全同构**，可作为缓冲池上限设计样板。
- **RingBuffer 结构**：`managed::RingBuffer` 环形缓冲 —— 对应 Novos `Socket.rx_buf/tx_buf` 环形缓冲（DESIGN §3.8）。
- **事件驱动 poll 模型**：给定时钟 + 设备帧 → 单次处理一回合 —— 对应 Novos softirq 下半部（DESIGN §1.4），可移植其"驱动回调推进协议状态机"的结构。
- **TCP 状态机 + 单元测试**：`src/iface/neighbor.rs`（ARP 缓存）、TCP socket 状态机有 netsim 测试 —— 对应 Novos §4.5 TCP 状态机单测。
- **PHY 抽象**：`Device` trait 定义 `receive/transmit` —— 对应 Novos `NetDevOps` trait。

**避坑**
- smoltcp 无 SACK/时间戳/select 全语义（它明确列为 anti-goal），是**基线而非完整栈** —— Novos 需要完整 TCP（重传、Cubic、epoll、wait queue），M5 用其协议分层结构，TCP 细节按 DESIGN §4.5 补。
- 单线程 poll 模型不适合多核；Novos UP 阶段无此问题。

### 4.2 其他网络参考

- **Redox smolnetd**（用户态网络 daemon，`ip/tcp/udp/icmp` schemes）：可看其分层，但 Novos 网络栈在内核态，仅作对照。
- **网关/NAT/conntrack**：Rust 内核态实现极少 —— 建议以 Linux conntrack 语义 + DESIGN §4.7 为准，用户态可参考 `nftnl-rs`/`nftables-rs` 的规则模型（但不引入）。

---

## 5. 容器 / 安全 / 设备类

### 5.1 youki —— OCI 容器运行时（M8/M14 首选参考）

- 仓库：<https://github.com/youki-dev/youki>
- 定位：Rust 实现的 OCI runtime（runc 替代），CNCF 沙箱项目，containerd/Podman/K8s 可集成。

**可借鉴（对应 Novos M8/M14 / DESIGN §4.6、§13）**
- **libcontainer**：`container/builder.rs`（容器创建参数 → 运行时状态）、`process/`（容器进程生命周期/信号）、`rootfs/`（rootfs 组装、overlay 挂载）—— **M14-06 containerd-like 守护进程的模块划分直接参考**。
- **libcgroups v2**：cgroup 树、memory/pids/cpu 控制器读写 —— 对应 Novos Cgroup 内核对象（DESIGN §3.5）的语义模型（内核侧 reimplement，行为对齐）。
- **seccomp / capabilities 应用**：容器创建时按 spec 设置 cap 集与 seccomp filter 的顺序与失败处理 —— 对应 M14-05。
- **生命周期状态机**：created/running/stopped 转换 + OOM 事件 → SIGKILL 流程 —— 对应 DESIGN §7.2 信号交互。

**避坑**
- youki 是**用户态**运行时（依赖 Linux 内核 syscall），Novos 的 namespace/cgroup 是**内核实现** —— 只借语义与状态机，**不借实现**（syscall 调用变成内核函数调用）。
- youki 依赖 libc/容器生态 crate，不可直接进内核。

### 5.2 oci-spec-rs —— OCI 规范类型（M14-04）

- 仓库：<https://github.com/youki-dev/oci-spec-rs>
- 借鉴点：`oci_spec::runtime::Spec`（process/rootfs/mounts/linux.capabilities/seccomp/namespaces）的完整类型模型 —— 直接作为 Novos 解析 `config.json` 的字段清单与校验规则（TASKS M14-04）。

### 5.3 seccompiler —— seccomp BPF（M12-10）

- 仓库：<https://github.com/firecracker-microvm/firecracker>（`src/seccompiler`）
- 定位：把 JSON 过滤规则编译为 BPF 字节码的编译器（Firecracker 生产使用）。

**可借鉴（对应 Novos M12 / DESIGN §13.5）**
- **规则语义**：`SeccompAction`（Allow/Errno/Kill/Trap/Log/Trace）、`filter_action`（白名单）与 `default_action` 分离 —— 与 Novos DESIGN §13.5 的 `SeccompAction` 枚举设计一致。
- **参数级过滤**：`args[{index,type,op,val}]`（eq/ne/ge/gt/lt/masked_eq）—— Novos 解释器虽只过滤 syscall number（`<500 行`），但参数匹配语义可扩展对照。
- **多线程分过滤器**：Firecracker 按 vmm/api/vcpu 线程分别加载 —— Novos 可按进程/容器粒度加载，思路相同。

**避坑**
- seccompiler 是"编译端"，Novos 需要的是"解释端"（BPF 字节码求值）—— 读它的 **BPF 指令生成逻辑**来理解语义，解释器自己写（DESIGN §13.5 已定 <500 行）。

### 5.4 virtio-drivers —— VirtIO guest 驱动（M0/M5/M10）

- 仓库：<https://github.com/rcore-os/virtio-drivers>
- 定位：Rust VirtIO guest 驱动（net/blk/gpu/input/rng），`no_std`，rCore/ArceOS 共用。

**可借鉴（对应 Novos M5/M10）**
- **Hal trait 抽象**：`dma_alloc/dma_dealloc/phys_to_virt/virt_to_phys` —— 正是 Novos virtio 驱动需要的最小 DMA 抽象（DESIGN §13.3 `BlockDevice` trait 同思路），**移植时把 Hal 换成 Novos 自己的页分配器**。
- **split VirtQueue**：`desc/avail/used` 三环 + `free_head` 空闲描述符链表 + `last_used_idx` —— 对应 TASKS M10-01 的 split descriptor ring，可直接对照实现。
- **virtio-blk**：`VirtIOBlk`（header + queue + capacity）与 `blk_size` 扇区语义 —— 对应 M10-01 `read_block/write_block/flush`。

**避坑**
- 该 crate 用 `dma` 内部抽象，Novos 需要与自身 page allocator/cgroup 记账对接（`charge/uncharge`），需改写 DMA 层。
- 版本迭代快，移植时锁定一个 tag 对照。

### 5.5 ext4 三件套（M10）

| 组件 | 仓库 | 定位 | 借鉴点 |
|---|---|---|---|
| ext4-view-rs | nicholasbishop/ext4-view-rs | `no_std` **只读** ext2/3/4 | 超级块/inode/dir/extent/htree 解析（M10-04/05 只读路径可直接对照），0.6.0 已支持 xattr 等 |
| rust-fs-ext4（am-fs-ext4） | christhomas/rust-fs-ext4 | 纯 Rust **读写** ext2/3/4 + JBD2 | 块分配、**JBD2 日志**（M10-06 data=journal 写路径 + 断电一致性测试方法的参考）、mkfs |
| mkext4 | cortexapps/mkext4 | 确定性 ext4 镜像构建 | 给测试造 ext4 磁盘镜像（M10-10 集成测试用），比 `mkfs.ext4` 更可控 |

**要点**：Novos M10 主线 = ext4-view-rs 的解析结构 + am-fs-ext4 的写/日志语义；两个仓库都有与 e2fsck 对齐的测试方法，可直接复用其 test 磁盘生成思路。

---

## 6. 按里程碑的快速参考清单

| 里程碑 | 首选参考 | 次要参考 |
|---|---|---|
| M0 引导/中断 | blog_os + bootloader + x86_64 crate | rCore 启动章节、Redox kernel |
| M1 物理内存/堆 | buddy_system_allocator、buddy-slab-allocator | buddy-alloc、smalloc、Redox mm/ralloc |
| M2 虚存/任务/调度 | rCore（页表+CFS）、intrusive-collections（RBTree） | Theseus（MappedPages）、RustyHermit（位图）、Hubris（确定性） |
| M3 syscall/init/shell | rCore syscall+ELF | zCore linux-syscall、arceos-runlinuxapp |
| M4 VFS/ramfs/tmpfs | ArceOS axfs_vfs/axfs_ramfs、rCore VFS | embed-collections（B+树目录）、Redox redoxfs |
| M5 网络栈 | smoltcp（结构+缓冲+状态机） | RustyHermit 网络、Redox smolnetd（对照） |
| M6 namespace/cgroup | youki libcgroups（语义） | rCore（pid 空间雏形） |
| M7 OverlayFS | 无 Rust 内核实现 → 读 Linux overlayfs 源码 | youki rootfs 层处理（用户态对照） |
| M8 容器+网关 | youki libcontainer（流程/状态机） | oci-spec-rs、DESIGN §4.6 |
| M9 稳定版/SMP | RustyHermit（per-core 队列）、Hubris（确定性） | Theseus（cell 审计） |
| M10 ext4/BIO/PageCache | virtio-drivers（blk）、ext4-view-rs、am-fs-ext4、mkext4 | ArceOS axdriver_block |
| M11 动态链接/futex/TLS | arceos-runlinuxapp（ELF+auxv+TLS）、zCore linux-object | Rust std futex（语义）、tokio sync |
| M12 设备/cap/seccomp | seccompiler（BPF 语义）、Tock（设备/生命周期） | getrandom crate（熵）、youki caps |
| M13 /proc/信号/timerfd | zCore / rCore procfs、tokio-time（分层时间轮） | smoltcp 定时器（对照） |
| M14 Docker/apt/JVM | youki（libcontainer/libcgroups）、oci-spec-rs | — |

---

## 7. 数据结构 / 算法速查（内核实现选型）

| Novos 需求 | 推荐数据结构/算法 | 参考来源 |
|---|---|---|
| 物理页分配（buddy） | 双向空闲链 + 分裂/合并 | buddy_system_allocator、Redox mm |
| 内核小对象（slab） | size-class + bitmap 对象位图 | buddy-slab-allocator（Linux SLUB 风格） |
| 调度 runqueue（vruntime） | 侵入式 RBTree，哨兵 nil + O(1) 最左节点 | intrusive-collections、intrusive-red-black-tree |
| VMA 区间管理 | 侵入式 RBTree（KeyAdapter=start） | intrusive-collections、Theseus |
| dcache / sk_buff LRU | 侵入式双向链表 | intrusive-collections |
| 目录项（getdents） | B+树（顺序遍历优） | embed-collections |
| fd 表 / inode 表 | 预分配 arena（token 句柄） | tokio-rs/slab |
| 定时器堆 | 分层时间轮（6 层）或最小堆 | tokio-time、DESIGN §4.5 |
| TCP 缓冲 | RingBuffer（环形） | smoltcp managed |
| conntrack 老化 | 定时器堆 + 链表 bucket | Linux conntrack 语义 |
| futex 等待队列 | 物理页 → 等待队列哈希表 | Rust std futex、DESIGN §13.7 |

---

## 8. 工程实践参考（过程性借鉴）

| 实践 | 参考来源 | Novos 落地 |
|---|---|---|
| unsafe 占比量化 | RustyHermit ≈3.27%（PLOS'19） | CI 断言 <5%（DESIGN §6.3⑥） |
| 内存预算可测 | Theseus cell 边界 + 预算 | DESIGN §5.3 `/proc/meminfo` + CI 断言 |
| 编译期缓冲上限 | smoltcp feature 常量 | sk_buff 池编译期上限 |
| LibOS 调试模式 | zCore（用户态跑内核） | 逻辑模块保留 host 可测接口 |
| 确定性内存核算 | Hubris 静态任务/内存 | 内核固定组件内存台账 |
| QEMU 集成测试 | blog_os / rCore 测试框架 | DESIGN §12.3 |
| 组件化 + 特性裁剪 | ArceOS axfeat | DESIGN §13.15 feature flags |
| ext4 一致性测试 | am-fs-ext4 断电模拟 | TASKS M10-06/M10-10 |

---

## 9. 参考链接汇总

### 完整内核 / OS
- Redox OS：https://github.com/redox-os/kernel
- Theseus：https://github.com/theseus-os/Theseus
- Tock：https://github.com/tock/tock
- ArceOS：https://github.com/arceos-org/arceos
- rCore：https://github.com/rcore-os/rCore
- zCore：https://github.com/rcore-os/zCore
- RustyHermit：https://github.com/hermit-os/kernel
- Hubris：https://github.com/oxidecomputer/hubris
- blog_os：https://github.com/phil-opp/blog_os
- bootloader：https://github.com/rust-osdev/bootloader
- x86_64：https://github.com/rust-osdev/x86_64

### 内存 / 数据结构
- buddy_system_allocator：https://github.com/rcore-os/buddy_system_allocator
- buddy-alloc：https://github.com/jjyr/buddy-alloc
- buddy-slab-allocator：https://github.com/weclaw1/buddy-slab-allocator
- smalloc：https://github.com/zooko/smalloc
- ralloc：https://github.com/redox-os/ralloc
- intrusive-collections：https://github.com/Amanieu/intrusive-rs
- embed-collections：https://github.com/ydrmaster/embed-collections
- slab：https://github.com/tokio-rs/slab

### 网络
- smoltcp：https://github.com/smoltcp-rs/smoltcp

### 容器 / 安全 / 设备 / FS
- youki：https://github.com/youki-dev/youki
- oci-spec-rs：https://github.com/youki-dev/oci-spec-rs
- firecracker（seccompiler）：https://github.com/firecracker-microvm/firecracker
- virtio-drivers：https://github.com/rcore-os/virtio-drivers
- ext4-view-rs：https://github.com/nicholasbishop/ext4-view-rs
- am-fs-ext4：https://github.com/christhomas/rust-fs-ext4
- mkext4：https://github.com/cortexapps/mkext4
- getrandom：https://github.com/rust-random/getrandom
- arceos-runlinuxapp：https://github.com/rcore-os/arceos-runlinuxapp

---

*本文档为调研参考，随实现推进持续修订。每个子系统落地前，建议先精读对应参考组件的相关目录，再动手。*
