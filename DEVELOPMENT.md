# Novos‑OS 开发步骤（路线图）

> 从“最小可启动内核”到“生产网关 + 多容器”，共 **10 个里程碑**。
> 每个里程碑：目标 → 任务 → 验收标准。**内存预算（≤32MB）从 M3 起每阶段校验，M9 正式达标。**

里程碑总览：

| # | 里程碑 | 内核预算（累计，估） | 前置依赖 |
|---|---|---|---|
| M0 | 最小可启动内核 + 串口 | 2–3 MB | — |
| M1 | 物理内存 + 内核堆 | 4–6 MB | M0 |
| M2 | 虚拟内存 + 任务 + 调度 | 8–12 MB | M1 |
| M3 | 系统调用 + 用户态 init/shell | 14–18 MB | M2 |
| M4 | VFS + ramfs/tmpfs | 18–22 MB | M3 |
| M5 | 网络栈 + socket + epoll | 24–28 MB | M4 |
| M6 | Namespace + Cgroup | 26–30 MB | M3（可与 M4/M5 并行） |
| M7 | OverlayFS | 28–31 MB | M4 + M6 |
| M8 | 容器运行时 + 网关 | 30–32 MB | M5+M6+M7 |
| M9 | 长期稳定版：压测/收敛/回填 | ≤32 MB | M8 |

**full 模式（Linux 兼容扩展，`--features full`）**

| # | 里程碑 | 内核预算（估） | 前置依赖 |
|---|---|---|---|
| M10 | Block I/O + ext4 + Page Cache | 34–36 MB | M9 |
| M11 | 动态链接 + futex + TLS | 36–38 MB | M10 |
| M12 | 设备框架 + Capabilities + Seccomp | 37–39 MB | M9（可与 M10/M11 并行） |
| M13 | 完整 /proc + 信号扩展 + 事件 fd | 38–39 MB | M11 |
| M14 | Docker 兼容 + apt install + JVM/Python | ≤40 MB | M10+M11+M12+M13 |

### 里程碑依赖关系图

```
M0 ──────┬──▶ M1 ──────┬──▶ M2 ──────┬──▶ M3 ──┬──▶ M4 ──────┬──▶ M5 ──────┐
最小启动  │  物理内存   │  虚拟内存   │ syscall │  VFS       │  网络栈    │
          │            │  +调度      │ +init   │  +ramfs    │  +epoll   │
          │            │            │ +shell  │            │           │
          │            │            │        │            │           │
          │            │            ├──▶ M6 ──┼────────────┼──▶ M7 ────┤
          │            │            │  Namespace│           │ OverlayFS │
          │            │            │  +Cgroup │           │           │
          │            │            │  (可并行) │           │           │
          │            │            │        │            │           │
          │            │            │        │            │           ▼
          │            │            │        │            │     M8 ◀───┘
          │            │            │        │            │  容器+网关
          │            │            │        │            │      │
          │            │            │        │            │      ▼
          │            │            │        │            │     M9 ◀──── minimal 1.0
          └────────────┴────────────┴────────┴────────────┘
                                                         │
                 ┌───────────────────────────────────────▼────────┐
                 │           full 模式扩展（--features full）        │
                 │                                                │
                 │  M10 ──────▶ M11 ──────────▶ M13 ──────────▶ M14 │
                 │  ext4+ BIO   动态链接+      完整/proc+       Docker+  │
                 │  +PageCache  futex+TLS     信号扩展        apt+JVM   │
                 │                    ▲                              │
                 │                    │                              │
                 │  M12 ──────────────┘  (可与 M10/M11 并行)          │
                 │  设备+Cap+Seccomp                              ≤40MB │
                 └────────────────────────────────────────────────────┘
```

### 参考组件速查（完整调研见 `REFERENCES.md`，DESIGN.md §14 有设计决策落地）

| 里程碑 | 首选参考 | 备选参考 |
|---|---|---|
| M0 引导/中断 | blog_os + rust-osdev（bootloader/x86_64/acpi） | rCore 启动章节 |
| M1 物理内存/堆 | buddy_system_allocator、buddy-slab-allocator | buddy-alloc、smalloc、Redox mm/ralloc |
| M2 虚存/任务/调度 | rCore（页表+CFS）、intrusive-collections | Theseus（MappedPages）、RustyHermit（位图）、Hubris |
| M3 syscall/init/shell | rCore syscall+ELF | zCore linux-syscall、arceos-runlinuxapp |
| M4 VFS/ramfs/tmpfs | ArceOS axfs_vfs/axfs_ramfs、rCore VFS | embed-collections（B+树目录）、Redox redoxfs |
| M5 网络栈 | smoltcp（结构+缓冲+状态机） | RustyHermit 网络 |
| M6 namespace/cgroup | youki libcgroups（语义） | rCore（pid 空间雏形） |
| M7 OverlayFS | 无 Rust 内核实现 → 读 Linux overlayfs 源码 | youki rootfs 层（用户态对照） |
| M8 容器+网关 | youki libcontainer（流程/状态机） | oci-spec-rs |
| M9 稳定版/SMP | RustyHermit（per-core）、Hubris（确定性） | Theseus（cell 审计） |
| M10 ext4/BIO/PageCache | virtio-drivers（blk）、ext4-view-rs、am-fs-ext4、mkext4 | ArceOS axdriver_block |
| M11 动态链接/futex/TLS | arceos-runlinuxapp（ELF+auxv+TLS）、zCore linux-object | Rust std futex、tokio sync |
| M12 设备/cap/seccomp | seccompiler（BPF 语义）、Tock（设备/生命周期） | getrandom、youki caps |
| M13 /proc/信号/timerfd | zCore/rCore procfs、tokio-time（分层时间轮） | smoltcp 定时器 |
| M14 Docker/apt/JVM | youki（libcontainer/libcgroups）、oci-spec-rs | — |

**并行策略**：M6（Namespace + Cgroup）只依赖 M3，可以与 M4（VFS）、M5（网络栈）并行推进，缩短总工期。M7（OverlayFS）依赖 M4 + M6，是 M8 的前置。full 模式中 M12（设备+安全）只依赖 M9，可与 M10/M11 并行。

---

## M0：最小可启动内核 + 串口输出

**目标**：能启动、能打印，作为所有后续的地基。

**任务**
- [ ] 搭建 Rust `no_std` 内核工程（`x86_64-unknown-none` target）；
- [ ] 编写链接脚本 `linker.ld`（段布局：`.text/.rodata/.data/.bss`，内核虚拟基址）；
- [ ] 引导：multiboot2（QEMU+GRUB）或 UEFI（`bootloader` crate），进入长模式；
- [ ] 初始化页表：内核直接映射（恒等映射或偏移映射）；
- [ ] 8250 UART 驱动：`print!`/`println!` 宏输出；
- [ ] 中断/异常处理：IDT + GDT，`panic` 落串口；
- [ ] 全局描述符、栈对齐、`#[panic_handler]`。

**验收**
- QEMU 启动后串口打印 `Novos-OS: boot ok` 与内存映射信息；
- 故意触发 panic 能打印寄存器快照、不重启（可调试）。

**工具链**
```bash
cargo new novos --lib
rustup target add x86_64-unknown-none
# bootloader + multiboot2 crates（或自写 boot）
```

---

## M1：物理内存 + 内核堆

**目标**：能分配/释放物理页与内核小对象。

**任务**
- [ ] 解析 bootloader 提供的物理内存映射（E820/memory map），构建 `PageFrame` 数组；
- [ ] Buddy 分配器（order 0–10）：分配、分裂、释放、合并；
- [ ] 内核堆：Slab 分配器（固定 size 阶梯 64B–4K）；
- [ ] `GlobalAlloc` 接入：`Box`/`Vec`/`Arc` 直接可用；
- [ ] 页帧引用计数 + 统计（`used/free` 计数）。

**验收**
- 单元测试：buddy 分配/释放无泄漏、无重叠；slab 对象复用正确；
- 能 `Vec::with_capacity(10_000)` 并释放，`used` 回落。

---

## M2：虚拟内存 + 任务 + 调度

**目标**：多任务并发切换。

**任务**
- [ ] 4 级页表管理：`mmap`/`munmap`、懒分配、COW；
- [ ] `Task` 结构（§3.3）+ 内核栈 + 上下文切换（`switch` 汇编）；
- [ ] CFS 简化调度器（§4.2）：vruntime 红黑树、tick 抢占；
- [ ] `fork`（克隆 Task + 复制页表，COW）、`exit`、`waitpid`；
- [ ] 同步原语：`Spinlock`/`Mutex`/`WaitQueue`；
- [ ] 定时器最小堆 + 时钟中断。

**验收**
- 内核线程调度：多个线程轮转执行、可睡眠唤醒；
- `fork` 后父子独立地址空间（COW 生效）。

---

## M3：系统调用 + 用户态 init/shell

**目标**：能跑用户态程序，命令行可用。

**任务**
- [ ] `syscall` 指令入口 + 参数解析 + 系统调用表（`read/write/open/close/exit/…`）；
- [ ] 用户态内存管理：`mmap`/`munmap`/`brk` 系统调用；
- [ ] ELF 加载器（静态链接 musl 二进制）；
- [ ] 第一版 libc 子集（或直接移植 musl 静态编译目标）；
- [ ] `init`（PID 1）+ 简易 `shell`（`fork`+`exec`+管道）；
- [ ] 文件描述符表、`/dev/uart` 设备文件。

**验收**
- `make run` 后进入 shell，能执行 `ls`/`cat`/`echo` 等内建/外部命令；
- **首次内存基线测量**：空闲内核常驻 ≤ 18MB（估算，作为对照锚点）。

---

## M4：VFS + ramfs/tmpfs

**目标**：完整文件系统抽象，为 OverlayFS 打底。

**任务**
- [ ] VFS 层：`SuperBlock/Inode/Dentry/File`（§3.6）；
- [ ] dcache：hash 查找 + LRU 可回收 + shrink_target；
- [ ] ramfs（initramfs 挂载）；
- [ ] tmpfs（页缓存文件，匿名页支持）；
- [ ] 系统调用：`open/read/write/close/stat/mkdir/rmdir/unlink/readdir/mount`；
- [ ] 路径解析（§4.3）+ 挂载点遍历。

**验收**
- 在 tmpfs 上完整读写/建目录/枚举目录；
- 删除文件后 `used` 内存回落（回收路径生效）。

---

## M5：网络栈 + socket + epoll

**目标**：TCP/IP 完整栈，能跑 HTTP。

**任务**
- [ ] 以太网 + virtio‑net 驱动；ARP；
- [ ] IPv4：收发、分片最小化、ICMP echo；
- [ ] UDP socket；
- [ ] TCP：三次握手、滑动窗口、重传、Cubic（或 NewReno）、TIME_WAIT（§4.5）；
- [ ] `socket/bind/listen/accept/connect/send/recv` 系统调用；
- [ ] `epoll_create/epoll_ctl/epoll_wait`（LT+ET，§3.8）；
- [ ] `NetNamespace` 雏形（loopback + 设备隔离）。

**验收**
- 内核内起 HTTP 服务，QEMU 内 `wget` 成功；
- 并发 100 个连接，epoll 正确唤醒、无丢包。

---

## M6：Namespace + Cgroup

**目标**：进程/资源隔离能力。

**任务**
- [ ] 7 种 namespace：pid/mnt/net/uts/ipc/user/cgroup（§3.4）；
- [ ] `clone` 带 flags → 创建 namespace；
- [ ] pid namespace：ns 内 pid=1，跨 ns 可见性；
- [ ] mount namespace：独立挂载视图；
- [ ] Cgroup v2 树 + memory/pids 控制器（§3.5）；
- [ ] `memory.max` 触发回收 / OOM‑kill（仅杀容器内进程）；
- [ ] `/proc` 只读视图（预算监控口）。

**验收**
- `clone` 新进程在不同 pid/uts/net ns，隔离生效；
- cgroup 限制 `memory.max=64MB`，超限进程被 kill，宿主无恙。

---

## M7：OverlayFS

**目标**：镜像层 + 可写层的文件系统。

**任务**
- [ ] `mount -t overlay`：lower/upper/work；
- [ ] 合并 lookup（§4.4）：upper→lower 逐层，whiteout 处理；
- [ ] copy‑up：写时提升 + workdir 原子 rename；
- [ ] 目录层级（不跨层时 zero‑copy 读）；
- [ ] `redirect_dir` 第一版禁用（省内存）。

**验收**
- 只读 lower（如镜像）上叠加可写 upper，写后下层不变；
- 删除下层文件产生 whiteout，重启后仍“已删除”。

---

## M8：容器运行时 + 网关

**目标**：跑起第一个容器 + 网关转发。

**任务**
- [ ] 类 runC 命令：`novos run <image> <cmd>`（§4.6 完整流程）；
- [ ] 容器 rootfs 用 OverlayFS 组装；`pivot_root`；
- [ ] 容器内挂 `/proc`（pid ns 视图）、设 cgroup；
- [ ] 网关：IP 转发 + conntrack + MASQUERADE + 端口映射（§4.7）；
- [ ] 基础防火墙（线性规则表）；
- [ ] init（PID 1）编排容器生命周期 + 回收僵尸。

**验收**
- `novos run busybox echo hello` 输出 hello，容器隔离正确；
- 容器外网访问经 MASQUERADE 成功，conntrack 老化回收；
- **内存基线**：空闲 32MB 内；跑 3 个容器内核常驻不涨破预算。

---

## M9：长期稳定版（压测 / 收敛 / 回填）

**目标**：32MB 基线正式达标，生产可用。

**任务**
- [ ] 内存回归测试套件：启动 N 容器 + 网关流量下断言常驻 ≤32MB；
- [ ] 每个子系统 shrink 路径压测（dcache/sk_buff/anon）；
- [ ] 编译期瘦身终检：`opt-level=s/z` + LTO + strip，回填实测 .text 大小；
- [ ] SMP 支持评估（若预算允许，加 per‑CPU 运行队列）；
- [ ] 稳定性：长跑 7 天无泄漏（used 不单调爬升）；
- [ ] 性能：容器启动延迟、网关吞吐达标值回填。

**验收**
- 文档 2.1/2.2 的每一行预算都有**实测数据支撑**；
- CI 中内存断言通过。

---

# 扩展里程碑（full 模式：Linux 兼容容器宿主）

> 以下 M10–M14 在 minimal 版（M0–M9）稳定后推进，通过 Cargo `--features full` 编译。
> 目标：支持 ext4、Docker 完整生态、`apt install`、JVM/Python。
> 内存预算调整为 **≤ 40MB**（见 DESIGN.md §13.14）。

| # | 里程碑 | 内核预算（估） | 前置依赖 |
|---|---|---|---|
| M10 | Block I/O + ext4 + Page Cache | 34–36 MB | M9 |
| M11 | 动态链接 + futex + TLS | 36–38 MB | M10 |
| M12 | 设备框架 + Capabilities + Seccomp | 37–39 MB | M9（可与 M10/M11 并行） |
| M13 | 完整 /proc + 信号扩展 + 事件 fd | 38–39 MB | M11 |
| M14 | Docker 兼容 + 容器服务（Redis + SQLite） | ≤40 MB | M10+M11+M12+M13 |

---

## M10：Block I/O + ext4 + Page Cache

**目标**：支持磁盘文件系统，为 apt 持久化打底。

**任务**
- [ ] `BlockDevice` trait + virtio-blk 驱动（§13.3）；
- [ ] BIO 层（简单队列 + 同步 I/O + 异步回调）；
- [ ] Page Cache（`AddressSpace`：文件偏移 → 物理页，可 shrink）；
- [ ] ext4 驱动（`FileSystemDriver` trait 实现）：超级块/inode/dir/extent/日志（最小化）；
- [ ] `mount -t ext4 /dev/vda /mnt`；
- [ ] `mmap MAP_SHARED` 文件映射（多进程共享物理页）；
- [ ] Page cache shrink：脏页回写 + 释放。

**验收**
- ext4 上创建/读写/删除文件，重启后持久化；
- 两个进程 mmap 同一 .so 文件 → 共享同一物理页（/proc/self/maps 验证）；
- **内存基线**：≤36MB。

---

## M11：动态链接 + futex + TLS + 工具链

**目标**：能跑动态链接的 ELF + pthread 线程；搭起 Go/Rust/C++ 自编译工具链与 ABI 契约（§15）。

**任务**
- [ ] ELF 加载器扩展：识别 `PT_INTERP` + `PT_DYNAMIC` + 设置辅助向量（AT_BASE/AT_PHDR/AT_RANDOM）（§13.6）；
- [ ] 加载 `ld-musl-x86_64.so.1` 动态链接器到地址空间；
- [ ] `mmap MAP_SHARED` 文件页映射（依赖 M10 Page Cache）；
- [ ] futex 系统调用（WAIT/WAKE/REQUEUE，按物理页索引等待队列）（§13.7）；
- [ ] TLS：`arch_prctl(ARCH_SET_FS)` + FS base MSR + 上下文切换恢复（§13.8）；
- [ ] clone 扩展：`CLONE_SETTLS` + `CLONE_CHILD_CLEARTID`；
- [ ] 移植 musl 动态链接版用户态二进制；
- [ ] 交叉工具链：musl-cross + crt1/crti/crtn + linker script + 版本锁定（§15.2）；
- [ ] ABI 契约文档：`docs/abi.md`（syscall 清单/结构体布局/errno/调用约定）；
- [ ] 自编译冒烟：Go（`CGO_ENABLED=0` 静态）、Rust（`x86_64-unknown-linux-musl`）、C++（`-static -static-libstdc++ -static-libgcc`）。

**验收**
- 运行动态链接的 hello world（`gcc -o hello hello.c` 不加 `-static`）；
- `pthread_create` 创建线程 + futex 互斥锁正常工作；
- `GOOS=linux` Go 静态二进制、`x86_64-unknown-linux-musl` Rust 静态二进制、musl 静态 C++ 二进制各自在 Novos 上运行输出；
- **内存基线**：≤38MB。

---

## M12：设备框架 + Capabilities + Seccomp

**目标**：Docker 安全模型 + 完整设备文件。

**任务**
- [ ] `CharDevice` trait + devtmpfs（§13.4）；
- [ ] 标准设备：`/dev/null`、`/dev/zero`、`/dev/urandom`、`/dev/random`；
- [ ] devpts：`/dev/ptmx` + `/dev/pts/N`（PTY 对 + 环形缓冲）；
- [ ] Capabilities：`TaskCreds`（permitted/effective/inheritable/bounding）+ 权限检查（§13.5）；
- [ ] Seccomp BPF：最小解释器（syscall number 过滤，<500 行）；
- [ ] `getrandom` 系统调用 + `/dev/urandom`（RDRAND）（§13.9）。

**验收**
- `echo hello > /dev/null` + `dd if=/dev/zero bs=4096 count=1`；
- `docker exec` 通过 PTY 进入容器交互；
- seccomp profile 禁止 `reboot` syscall → 容器内调 reboot 返回 EPERM；
- **内存基线**：≤39MB。

---

## M13：完整 /proc + 信号扩展 + 事件 fd

**目标**：动态链接程序（busybox/Go/SQLite 等 musl 二进制）可观测性 + 完整信号。

**任务**
- [ ] /proc 扩展：`/proc/self/maps`、`/proc/self/status`、`/proc/self/exe`、`/proc/self/fd/`（§13.12）；
- [ ] `/proc/cpuinfo`、`/proc/mounts`、`/proc/filesystems`；
- [ ] 信号扩展：`sigaction`（SA_SIGINFO + SA_ONSTACK + SA_RESTART）、`sigprocmask`、`sigaltstack`（§13.10）；
- [ ] 信号到 64 个（实时信号）；
- [ ] timerfd：`timerfd_create` + `timerfd_settime` + epoll 可监听（§13.11）；
- [ ] signalfd（可选，事件循环库用）。

**验收**
- 动态链接 busybox（musl）启动后 `/proc/self/maps` 输出正确的地址空间布局；
- 动态程序捕获 SIGSEGV 正常（`sigaltstack` + SA_SIGINFO）；
- timerfd 到期后 epoll_wait 正确唤醒；
- **内存基线**：≤39MB。

---

## M14：Docker 兼容 + 真实容器服务（Redis + SQLite）

> 定位调整：**演示目标避开 glibc 陷阱**（见 DESIGN.md §14 与 REFERENCES.md）。
> `apt/JVM/Python` 是 glibc 生态，价值递减、成本陡增，降为 **P3 可选**；
> full 模式以"跑真实 musl 容器服务"为验收锚点。

**目标**：full 模式正式达标——docker 兼容 CLI 跑起 **Redis + SQLite** 容器服务，外部可访问。

**任务**
- [ ] veth pair + bridge 虚拟网络设备（Docker CNI 默认网络）（§13.11）；
- [ ] 完整 DNAT 端口映射规则（`docker -p 8080:80`）；
- [ ] OCI runtime spec 兼容（config.json 解析 + seccomp/capability 应用）；
- [ ] containerd-like 守护进程（容器生命周期管理 + image pull）；
- [ ] **最小记录锁**：`fcntl(F_SETLK/F_GETLK/F_UNLCK)` 字节区间锁（SQLite 依赖，DESIGN §6.2①）；
- [ ] 移植 **SQLite**（musl 静态 `libsqlite3.a`，`SQLITE_THREADSAFE=0`）：CRUD + WAL 持久化；
- [ ] 移植 **Redis**（musl 编译）：`SET/GET`、AOF/RDB、外部 TCP 访问；
- [ ] HTTPS 下载支持（用户态 TLS，内核 TCP 通道）——image pull 与 P3 包管理共用；
- [ ] （P3 可选）apt + dpkg 移植（动态链接 musl 版）；
- [ ] （P3 可选）OpenJDK / CPython 移植（需评估 musl 构建，glibc 陷阱风险高）。

**验收**
- `docker run redis` 容器启动，外部 `redis-cli SET/GET` 通过端口映射可访问；
- 容器内 SQLite 建表/增删改查 + 重启后数据持久化（ext4）；
- `docker run busybox echo hello` 完整跑通（veth + bridge + overlay + seccomp）；
- 自编译的 Go / Rust / C++ 静态二进制作为容器进程运行（§15 工具链闭环）；
- **内存基线**：≤40MB（full 模式最终断言）。

---

## 开发节奏建议

- 每个里程碑独立可运行，**先跑通再优化**（YAGNI）；
- 内存预算只在**里程碑验收**时测量，开发中不打断主线；
- 建议维护 `docs/bench/` 记录每次基线测量（kernel .text / 空闲 used / 峰值）；
- 里程碑间可并行：M6 不依赖 M4/M5，可与 M4/M5 并行推进。

## 测试策略

- 单元测试：buddy/slab/rbtree/路径解析/TCP 状态机（`cargo test` 在 host 跑，逻辑与硬件解耦）；
- 集成测试：QEMU 启动断言（串口日志 + 内存统计）；
- 回归测试：CI 中 M9 内存断言。

### 各里程碑测试细化

| 里程碑 | 单元测试 | 集成测试 | 内存基线 |
|---|---|---|---|
| M0 | 无（无逻辑可测） | QEMU 串口输出 `boot ok` + panic 快照 | 仅记录 .text 大小 |
| M1 | buddy 分配/释放/合并；slab 复用/回收 | QEMU 内 `Vec::push` 不 panic | 记录 free/used 页数 |
| M2 | rbtree 插入/最左；上下文切换不丢寄存器 | 多内核线程轮转 + fork COW 验证 | — |
| M3 | syscall 参数解析；ELF 段加载 | shell 跑 `ls`/`echo`/`cat` | **首次基线**：≤18MB |
| M4 | 路径解析；dcache 命中/miss | tmpfs 读写 + 删除后内存回落 | dcache shrink 断言 |
| M5 | TCP 状态机转换；epoll 就绪队列 | QEMU 内 HTTP + 100 连并发 | sk_buff 上限断言 |
| M6 | pid ns 层级；cgroup 记账 | clone 隔离 + `memory.max=64MB` OOM kill | cgroup 管理对象计数 |
| M7 | overlay lookup；copy-up 流程 | 写后下层不变 + whiteout 持久 | overlay cache 占用 |
| M8 | 容器创建全流程（§4.6）；NAT 规则匹配 | `novos run busybox echo hello` + 外网 | **3 容器 ≤32MB** |
| M9 | 全回归 | 7 天长跑 + 1000 连接 burst | **≤32MB 最终断言** |
| M10 | BIO 队列；ext4 inode 操作 | ext4 读写 + 重启持久化 | ≤36MB |
| M11 | futex wait/wake；ELF 动态段解析 | 动态 hello world + pthread | ≤38MB |
| M12 | seccomp BPF eval；cap 检查 | docker exec PTY + seccomp 拦截 | ≤39MB |
| M13 | /proc/self/maps 格式；sigaltstack | JVM /proc + SIGSEGV 捕获 | ≤39MB |
| M14 | Docker 全流程；apt 依赖链 | `docker run` + `apt install` + `java -version` | **≤40MB full 最终断言** |

---

## 风险评估与缓解

| 风险 | 概率 | 影响 | 缓解策略 |
|---|---|---|---|
| **TCP 栈复杂度超预期** | 高 | M5 延期，吃掉后续预算 | 先实现 NewReno（代码更小），Cubic 留到 M9；TCP 状态机用单元测试先行验证 |
| **32MB 预算超标** | 中 | M9 无法达标 | 从 M3 起每阶段测量基线，超标即排查（不靠编译选项糊弄）；shrink 路径优先实现 |
| **Rust `no_std` crate 缺失** | 低 | 某些子系统需自实现 | 网络栈/TCP 等核心模块自实现（不依赖外部 crate），基础工具 crate（spin/bitflags）已成熟 |
| **OverlayFS copy-up 正确性** | 中 | 容器写数据丢失 | 大量边界测试：并发写、大文件分块、白名单覆盖、原子 rename 中断恢复 |
| **Cgroup OOM 误杀** | 低 | 容器进程被错误 kill | 记账路径用原子计数 + 单元测试验证 `charge`/`uncharge` 配对 |
| **中断处理性能瓶颈** | 中 | 网络吞吐低 | softirq 下半部延迟处理 + sk_buff 批量投递 |
| **单核调度饥饿** | 低 | 低优先级进程不运行 | vruntime clamp 防饿死 + 定期测试验证公平性 |

---

## CI/CD 流水线

### 流水线阶段

```
PR 提交
  │
  ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  Stage 1     │───▶│  Stage 2     │───▶│  Stage 3     │───▶│  Stage 4     │
│  格式检查     │    │  单元测试     │    │  QEMU 集成   │    │  内存回归    │
│              │    │              │    │              │    │              │
│ · cargo fmt  │    │ · cargo test │    │ · 启动断言    │    │ · kernel_used│
│ · clippy     │    │ · 全模块      │    │ · shell 命令  │    │   ≤32MB     │
│ · 编译无警告  │    │              │    │ · 内存统计    │    │ · shrink 断言│
└──────────────┘    └──────────────┘    └──────────────┘    └──────────────┘
                                                                  │
                                                                  ▼
                                                          ┌──────────────┐
                                                          │  Stage 5     │
                                                          │  文档检查     │
                                                          │              │
                                                          │ · bench 回填  │
                                                          │ · DESIGN 同步 │
                                                          └──────────────┘
```

### GitHub Actions 配置（参考）

```yaml
name: CI
on: [push, pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          targets: x86_64-unknown-none
          components: rust-src, clippy, rustfmt
      - name: fmt
        run: cargo fmt --all -- --check
      - name: clippy
        run: cargo clippy -- -D warnings
      - name: unit tests
        run: cargo test --workspace
      - name: install qemu
        run: sudo apt install -y qemu-system-x86
      - name: build kernel
        run: make build
      - name: qemu integration
        run: make test-integration
      - name: memory regression
        run: make test-memory
```

### 跳过策略

- 纯文档变更（`docs/` 目录）→ 跳过 QEMU 集成和内存回归（标记 `[ci skip-build]`）；
- 单模块改动 → 只跑相关模块的单元测试（路径过滤）。

---

## 代码审查清单

PR 合并前，审查者逐项确认：

### 通用项
- [ ] `cargo fmt` + `cargo clippy -D warnings` 通过
- [ ] 公开 API 有 `///` 文档注释
- [ ] `unsafe` 块有 `// SAFETY:` 不变量说明
- [ ] 无 `unwrap()`/`expect()` 在非初始化路径（用 `?` 传播错误）
- [ ] 无 `todo!()`/`unimplemented!()` 残留

### 内存相关
- [ ] 新增 `Arc`/`Box`/`Vec` 的分配路径不引入循环引用（`Weak` 破环）
- [ ] 缓存类结构（dcache/icache/sk_buff）有 shrink 路径或水位上限
- [ ] 页分配路径有 `charge`/`uncharge` 配对（Cgroup 记账）
- [ ] 无 `unsafe` 直接操作物理页帧绕过 buddy

### 并发相关
- [ ] 中断上下文（`#[interrupt]`）只用 `Spinlock`，不用 `Mutex`
- [ ] 锁获取顺序一致（避免死锁：mm → fs → net → sched）
- [ ] 无 `while !lock.try_lock() {}` 自旋（用 `lock()` 或 `WaitQueue`）

### 安全相关
- [ ] syscall 入口校验用户态指针（`copy_from_user`/`copy_to_user`）
- [ ] 无内核地址直接暴露到用户态（无 `/proc/kallsyms` 等价物）
- [ ] 容器隔离路径（clone/pivot_root/cgroup）按 §4.6 流程，无捷径

---

## 发布策略

### 版本号

遵循 SemVer：`MAJOR.MINOR.PATCH`

- **MAJOR**：ABI 破坏性变更（syscall 编号/参数语义变更）
- **MINOR**：新里程碑完成（M0–M9 对应 0.1–0.9）
- **PATCH**：bug 修复、性能优化（不影响 ABI）

| 里程碑 | 版本 | 阶段 |
|---|---|---|
| M0–M2 | 0.1–0.2 | alpha（内核自测） |
| M3–M5 | 0.3–0.5 | beta（用户态可用） |
| M6–M7 | 0.6–0.7 | rc（容器隔离可用） |
| M8 | 0.8 | rc（容器+网关跑通） |
| M9 | 1.0 | **正式发布（minimal）**（32MB 达标 + 稳定） |
| M10–M11 | 1.1–1.2 | full alpha（ext4 + 动态链接） |
| M12–M13 | 1.3–1.4 | full beta（Docker 安全模型 + 完整信号） |
| M14 | 2.0 | **正式发布（full）**（40MB 达标 + Docker/apt/JVM/Python 可用） |

### 发布物

每个版本产出：
1. `novos-kernel-x86_64.bin`——内核镜像（release + strip）；
2. `novos-initramfs.cpio`——initramfs（init + shell + container runtime）；
3. `docs/bench/<version>/`——实测内存数据回填；
4. `CHANGELOG.md`——变更记录。

### 回滚策略

- 每个 PR 必须独立可 revert（不依赖其他未合并 PR）；
- 内存回归失败 → 自动 block PR，不允许合并；
- 版本发布后发现严重 bug → 回退到上个 PATCH 版本，不就地 hotfix（保持线性历史）。
