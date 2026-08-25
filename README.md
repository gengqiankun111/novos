# Novos‑OS

> 用 Rust 从零编写，面向 **网关 + 容器** 场景的操作系统内核。
> 目标：**常驻内存 ≤ 32MB** 即可稳定运行容器工作负载 —— 一个比裁剪后主线 Linux 更小、更可控的容器宿主内核。

Novos‑OS 不是 Linux 的复制品。它砍掉了数十年来的兼容包袱和成千上万个用不到的驱动，只保留容器基础设施必须的最小闭环：**TCP/IP 完整网络栈 + epoll + Namespace + Cgroup + OverlayFS + 调度/内存管理**。

---

## 项目背景与动机

### 为什么不用裁剪版 Linux？

裁剪 Linux 看似省事，但实际有三个结构性问题：

1. **内存地板太高**——即使把模块和驱动剥到最精简，主线 Linux 的 idle 常驻仍在 50–80MB。原因是数十年积累的兼容层、通用框架开销（VFS 层抽象、block layer、netfilter 框架、procfs/sysfs 基础设施）无法通过配置项移除，它们已深入编译期依赖链。
2. **可审计性差**——想确认"这台机器的内核到底占多少内存、哪些子系统在吃"，需要穿过 Kconfig、defconfig、运行时 `slabtop`、`meminfo` 多层间接。预算无法在编译期固化。
3. **安全攻击面大**——Linux 有数千万行代码、成百上千个 syscall，每年 CVE 持续增长。容器宿主内核只需要其中一小部分，但无法裁掉不需要的攻击面。

Novos‑OS 选择从零构建，用 Rust 的内存安全保证 + 极小代码量 + 工程化内存预算，从根源上解决这三个问题。

### 为什么选 Rust？

| 维度 | C（传统内核） | Rust（Novos‑OS） |
|---|---|---|
| 内存安全 | 依赖人工审查，use-after-free / buffer overflow 持续产生 CVE | 所有权 + 借用检查在编译期消除整类内存 bug |
| 并发安全 | lock ordering 靠人记，data race 难以检测 | `Send`/`Sync` trait 在编译期保证线程安全 |
| 抽象零成本 | 宏 + 函数指针，泛型靠手写或宏膨胀 | trait + 泛型单态化，无运行时开销 |
| 错误处理 | 返回值 errno，容易遗漏 | `Result<T, E>` + `?` 强制处理 |
| 生态 | 大量成熟 crate 可复用 | 部分领域 crate 尚不成熟，但 `no_std` 生态已可用 |

### 设计哲学

三条原则贯穿所有设计决策：

1. **只保留容器基础设施的完整闭环**——TCP/IP 完整网络栈、epoll、Namespace、Cgroup、OverlayFS 一个都不能少，其余（图形、大量驱动、兼容层）一律砍掉。
2. **内存按预算工程化**——每个子系统有明确的预算上限，超预算视为 bug。缓存类内存（dcache/icache/sk_buff）必须可回收、可 shrink。
3. **性能让位于确定性**——在 32MB 内追求可预测的延迟和可控的抖动，而不是极限吞吐。

```
┌────────────────────────────────────────────────────────────┐
│                       Novos‑OS                              │
│                                                            │
│  ┌──────────────┐   ┌──────────────┐   ┌───────────────┐   │
│  │  init / shell │   │  容器运行时    │   │   网关用户态   │   │
│  └──────┬───────┘   └──────┬───────┘   └───────┬───────┘   │
│         └────────────┬─────┴───────────────────┘           │
│  ┌───────────────────▼───────────────────────────────────┐ │
│  │        系统调用层（syscalls / VFS / socket）            │ │
│  ├───────────────────────────────────────────────────────┤ │
│  │  进程调度 │ 内存管理 │ VFS │ 网络栈 │ Namespace/Cgroup │ │
│  ├───────────────────────────────────────────────────────┤ │
│  │             OverlayFS │ 驱动（virtio/uart/timer）       │ │
│  ├───────────────────────────────────────────────────────┤ │
│  │                   内核核心（x86‑64 / aarch64）          │ │
│  └───────────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

---

## 目标场景

Novos‑OS 通过 **Feature Flag** 支持两种部署形态：

| 模式 | 内核常驻 | 定位 | 适用场景 |
|---|---|---|---|
| **minimal** | ≤ 32MB | 最小容器宿主 | 边缘网关、IoT 节点、资源受限设备 |
| **full** | ≤ 40MB | Linux 兼容容器宿主 | 支持 ext4、Docker 完整生态、apt install、JVM/Python |

### minimal 模式（32MB）

| 场景 | 描述 | 为什么选 Novos‑OS |
|---|---|---|
| **边缘容器网关** | 工业网关/IoT 边缘节点上运行容器化的数据采集 + 网络转发 | 32MB 内核常驻 → 留更多内存给业务容器 |
| **轻量容器宿主** | 小型 VPS / 嵌入式设备上跑 2–5 个容器 | 比裁剪 Linux 内存更低，部署更可控 |
| **安全隔离网关** | 需要极小可信计算基（TCB）的网络转发 + 容器隔离 | 从零内核 + Rust → 代码量小、可审计、攻击面窄 |

### full 模式（~40MB）

| 场景 | 描述 | 扩展能力 |
|---|---|---|
| **Linux 兼容容器宿主** | 跑标准 Docker 容器 + `apt install` 包管理 | 动态链接 + ext4 + 完整 /proc + devpts |
| **开发/运维环境** | 容器内跑 JVM / Python / Node.js 应用 | futex + TLS + 完整信号 + getrandom |
| **CI/CD 平台** | 高密度容器调度 + 动态语言测试 | seccomp + capabilities + CNI 网络 |

### 不适合的场景

- **桌面/图形工作站**——无 GUI、无声卡、无 GPU 栈；
- **通用 Linux 发行版替代**——不兼容 Linux ABI 之外的程序（不提供 glibc 兼容层）；
- **高吞吐存储服务器**——无完整 block layer、无 RAID、无文件系统生态（只有 OverlayFS + tmpfs/ramfs）。

---

## 与同类项目对比

| 维度 | 裁剪 Linux (LinuxKit) | OSv (Unikernel) | MirageOS | **Novos‑OS** |
|---|---|---|---|---|
| 定位 | 通用容器宿主 | 单应用 unikernel | 库操作系统（OCaml） | **网关 + 容器宿主内核** |
| 语言 | C | C++ | OCaml | **Rust** |
| 内存安全 | 否 | 部分 | 是（GC 语言） | **是（编译期保证）** |
| 常驻内存 | 50–80 MB | 20–30 MB | 10–20 MB | **≤ 32 MB** |
| 多容器支持 | 是（完整） | 否（单应用） | 否（单应用） | **是（Namespace + Cgroup）** |
| 网络栈 | 完整 Linux | 简化 TCP | 协议库组合 | **完整 TCP/IP + epoll** |
| OverlayFS | 是 | 否 | 否 | **是** |
| 网关能力 | netfilter/iptables | 无 | 无 | **NAT + conntrack + 防火墙** |
| Linux 兼容性 | 完全 | 部分（单进程） | 无 | **musl 静态链接兼容** |

> OSv/MirageOS 更小但牺牲了多容器隔离；LinuxKit 通用但太重。Novos‑OS 在"能跑多容器"和"常驻够小"之间找到了目标定位。

---

## 目录结构（规划）

```
novos/
├── kernel/                 # 内核核心
│   ├── arch/               # 架构相关：x86_64/ aarch64/
│   │   ├── x86_64/         # 启动、页表、中断、上下文切换
│   │   └── aarch64/        # （后续支持）
│   ├── mm/                 # 内存管理：buddy/slab/vmalloc/page_table
│   ├── sched/              # 调度器：CFS 简化、运行队列
│   ├── syscall/           # 系统调用入口、分发表
│   ├── fs/                 # VFS + ramfs/tmpfs/overlayfs
│   ├── net/                # 网络栈：L2→L4 + epoll + conntrack
│   ├── ns/                 # Namespace：pid/mnt/net/uts/ipc/user/cgroup
│   ├── cgroup/             # Cgroup v2 控制器
│   ├── sync/               # 同步原语：spinlock/mutex/waitqueue
│   ├── driver/             # 驱动：virtio/uart/timer
│   └── lib.rs              # 内核入口
├── user/                   # 用户态
│   ├── init/               # PID 1
│   ├── shell/              # 命令行 shell
│   ├── container/          # 类 runC 容器运行时
│   └── gateway/            # 网关控制面
├── libs/                   # 共享库（no_std）
│   ├── bitmap/             # 位图工具
│   ├── rbtree/             # 红黑树
│   └── slab/               # Slab 分配器接口
├── tests/                  # 集成测试
│   ├── qemu/               # QEMU 启动断言
│   └── bench/              # 内存基线测量脚本
├── docs/                   # 文档
│   ├── DESIGN.md           # 设计文档
│   ├── DEVELOPMENT.md      # 开发路线图
│   └── bench/              # 每次基线测量记录
├── Makefile                # 构建入口
├── Cargo.toml              # workspace 根
└── README.md               # 本文件
```

---

## 技术选型

| 选项 | 决策 | 理由 |
|---|---|---|
| 开发语言 | **Rust nightly** | 内存安全 + 零成本抽象 + `no_std` 生态 |
| 目标三元组 | `x86_64-unknown-none` | 不依赖宿主 OS，裸金属 |
| 引导方式 | multiboot2（第一版）/ UEFI（后续） | QEMU + GRUB 快速迭代 |
| 用户态 libc | **musl（静态链接）** | 体积小、不依赖动态加载器、兼容常见 ELF 二进制 |
| 构建系统 | **Cargo + Makefile** | Cargo 管依赖，Makefile 封装 QEMU/测试命令 |
| 测试框架 | `cargo test`（host 逻辑） + QEMU 集成 | 逻辑与硬件解耦，CI 可跑 |
| CI | GitHub Actions | 编译 + `cargo test` + QEMU 启动断言 |
| 模拟器 | **QEMU** | 免费、支持 virtio、GDB 调试 |

### 关键依赖 crate（规划）

| crate | 用途 | 是否 `no_std` |
|---|---|---|
| `bootloader` | UEFI/multiboot2 引导 | 是 |
| `spin` | 自旋锁原语 | 是 |
| `bitflags` | 标志位类型 | 是 |
| `linked_list_allocator` | 内核堆分配（初期） | 是 |
| `volatile` | MMIO 寄存器访问 | 是 |
| `lazy_static` | 全局静态初始化 | 是 |
| `x86_64` | x86_64 汇编封装（页表/IDT/GDT） | 是 |
| `raw-cpuid` | CPU 特性检测 | 是 |

> 不引入 `std` 依赖的 crate；所有 crate 必须 `no_std` 兼容。

---

## 设计目标（内存）

### 🎯 第一版：无桌面、命令行、具备容器能力

**常驻物理内存 ≤ 32MB**，拆解：

| 组成部分 | 预算 | 说明 |
|---|---|---|
| 内核代码 + 静态数据 | **8–12 MB** | Rust 编译优化（`opt-level=s/z` + LTO + strip） |
| 内核运行开销 | **12–16 MB** | 页表、调度、VFS、网络栈、Namespace/Cgroup、OverlayFS 缓存 |
| 用户态基础程序 | **4–6 MB** | init、shell（静态链接 + strip） |

> 对比：裁剪后的主线 Linux 空闲约 **50–80MB**。Novos‑OS 更小，因为它没有几十年遗留兼容层，也不需要加载成千上万的无用驱动。

### 🎯 长期稳定版：生产网关主机 + 多个 Docker 容器

- 内核基础开销**依然保持 ≤ 32MB**；
- 容器内存**完全独立核算**，不计入内核常驻开销 —— Namespace/Cgroup 只是内核管理逻辑，容器应用消耗的内存另算；
- 网关能力（NAT/conntrack/转发）按连接数动态分配、可回收，不显著抬升常驻基线。

### ⚠️ 明确不追求的目标

**不要强行压到 8–16MB。** 因为必须携带：TCP/IP 完整网络栈、epoll、cgroup、namespace、overlayfs。这些容器基础设施本身有固定内存开销，过度精简只会牺牲性能，得不偿失。

---

## 特性清单

### minimal 模式（核心闭环）

- [x] 自举 / 启动引导（multiboot2 / UEFI）
- [ ] 物理内存管理（Buddy + Slab）
- [ ] 虚拟内存（4 级页表、lazy 分配、COW）
- [ ] 抢占式调度（CFS 简化实现）
- [ ] 系统调用 + 用户态 init/shell
- [ ] VFS + ramfs/tmpfs + OverlayFS
- [ ] TCP/IP 完整网络栈 + epoll
- [ ] 7 种 Namespace（pid/mnt/net/uts/ipc/user/cgroup）
- [ ] Cgroup v2（memory/cpu/pids/io）
- [ ] OverlayFS（lower/upper/work + copy‑up）
- [ ] 容器运行时（类 runC 命令）
- [ ] 网关（NAT/路由/基础防火墙）

### full 模式扩展特性（Linux 兼容）

- [ ] ext4 磁盘文件系统（Block I/O 层 + Page Cache）
- [ ] 动态链接（ELF PT_INTERP + MAP_SHARED + ld.so）
- [ ] futex（pthread 基础设施）
- [ ] TLS（FS/GS 段基址 + arch_prctl）
- [ ] Linux Capabilities（~15 种常用 cap）
- [ ] Seccomp BPF（Docker 安全 profile）
- [ ] devtmpfs + devpts（/dev/null/zero/urandom + PTY）
- [ ] 完整 /proc（/proc/self/maps、exe、fd/、status）
- [ ] getrandom + /dev/urandom
- [ ] 完整信号（sigaction/sigprocmask/sigaltstack + SA_SIGINFO）
- [ ] timerfd / signalfd（事件循环）
- [ ] veth/bridge（Docker CNI 网络）
- [ ] `apt install` 支持（动态链接 + FHS + HTTPS）
- [ ] JVM / Python 运行时支持

---

## 快速构建（规划）

```bash
# 需要 nightly Rust + 目标平台工具链
rustup toolchain install nightly
rustup target add x86_64-unknown-none

# 构建内核镜像——minimal 模式（32MB）
make build
# 构建内核镜像——full 模式（~40MB，Linux 兼容）
make build-full   # = cargo build --release --features full

# 运行到 QEMU
make run
```

构建要求与命令以各里程碑的实际推进为准（见 [开发步骤](docs/DEVELOPMENT.md)）。

---

## 文档索引

| 文档 | 内容 |
|---|---|
| [docs/DESIGN.md](docs/DESIGN.md) | 设计文档：架构、内存预算拆解、数据结构、核心算法 |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | 开发路线图：M0–M9 里程碑、任务拆解、验收标准 |

---

## 路线图（速览）

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M0 | 最小可启动内核 + 串口输出 | 能在 QEMU 打印日志 |
| M1 | 物理内存 + 内核堆 | buddy/slab 可用 |
| M2 | 虚拟内存 + 任务 + 调度 | 多线程切换 |
| M3 | 系统调用 + 用户态 init/shell | 能跑 shell 命令 |
| M4 | VFS + ramfs/tmpfs | 文件读写 |
| M5 | 网络栈 + socket + epoll | TCP 连接、HTTP 请求 |
| M6 | Namespace + Cgroup | 隔离进程、限内存 |
| M7 | OverlayFS | 容器镜像挂载 |
| M8 | 容器运行时 + 网关 | 跑起第一个容器 |
| M9 | 内存基线 ≤32MB + 长期稳定版 | 生产可用 |

---

## License

TBD（建议 GPL‑2.0 或 MIT，取决于是否参考 Linux 头文件语义）。

---

## 贡献指南

### 开发环境准备

```bash
# 1. 安装 nightly Rust
rustup toolchain install nightly
rustup default nightly

# 2. 添加裸金属目标
rustup target add x86_64-unknown-none

# 3. 安装 QEMU（x86_64）
#    Ubuntu:  sudo apt install qemu-system-x86
#    macOS:   brew install qemu
#    Windows: 参见 QEMU 官方安装指南

# 4. 安装 musl 工具链（用户态交叉编译）
#    Ubuntu:  sudo apt install musl-tools

# 5. 克隆并构建
git clone <repo-url>
cd novos
make build
make run
```

### 代码规范

- `cargo fmt` 格式化全部代码；
- `cargo clippy -- -D warnings` 零警告通过；
- 公开 API 必须有 `///` 文档注释；
- `unsafe` 块必须配 `// SAFETY:` 注释说明不变量；
- 提交信息格式：`<type>(<scope>): <subject>`（type: feat/fix/docs/refactor/test/chore）。

### 提交流程

1. 从 `main` 拉分支：`feat/<milestone>-<short-desc>`；
2. 确保本地 `make test` 全绿（单元 + QEMU 集成）；
3. PR 关联对应里程碑的 Issue；
4. 代码审查通过后合并到 `main`。

### 内存预算审查

任何 PR 如涉及以下子系统，必须附带内存影响说明：

- `mm/`——分配器逻辑变更
- `fs/`——缓存策略变更
- `net/`——缓冲池/连接数变更
- `cgroup/`——记账逻辑变更

说明格式：`内存影响: +0.2MB (sk_buff 池扩大上限)`。

---

## FAQ

**Q1：为什么不用 seccomp + AppArmor 裁剪 Linux，而要从零写内核？**

seccomp/AppArmor 裁剪的是运行时行为，不改变编译期代码量和内核常驻内存。主线 Linux 的 VFS 层、block layer、netfilter 框架等通用基础设施无论怎么裁剪配置都无法移除。从零写内核才能把"不需要的代码"在编译期完全删除。

**Q2：Rust `no_std` 生态成熟吗？能写出完整内核吗？**

`no_std` 生态已覆盖内核开发所需的基础设施：`spin`/`lazy_static`/`bitflags`/`volatile`/`x86_64` 等 crate 已被多个教学内核项目验证（`blog_os`、`Theseus`、`Redox`）。网络栈部分（TCP 状态机、sk_buff 管理）逻辑复杂但可自实现，不需要依赖外部 crate。

**Q3：能跑标准 Docker 镜像吗？**

第一版目标是跑 musl 静态链接的 ELF 二进制（如 busybox 静态版）。标准 Docker 镜像依赖 glibc 动态链接和完整的 OCI runtime，需要更多兼容工作。长期目标是兼容 OCI image spec。

**Q4：32MB 够用吗？容器自己也要内存啊。**

32MB 是**内核常驻开销**，容器应用消耗的内存完全独立——通过 Cgroup `memory.max` 逐容器隔离，不计入内核预算。32MB 只管"内核自己吃多少"，不管容器业务用多少。

**Q5：支持多核（SMP）吗？**

第一版**单核**。SMP 引入 per-CPU 运行队列、IPI、RCU 等复杂度，会吃掉内存预算。SMP 留到 M9 稳定后评估，前提是内存预算不被打破。

**Q6：为什么不实现 swap？**

swap 需要磁盘 I/O 栈 + block layer + 交换映射表，这些在 32MB 预算内装不下。容器场景下 swap 会引入不可控延迟，与"确定性优先"原则冲突。内存不足时靠 Cgroup OOM-kill 容器内进程，不波及宿主。

**Q7：和 Redox OS 有什么区别？**

Redox 是通用微内核 OS，有窗口系统、包管理器和用户生态。Novos‑OS 是专用内核，只解决"容器宿主 + 网关"，砍掉一切与该场景无关的子系统。目标内存预算（32MB vs Redox ~200MB+）和定位完全不同。

**Q8：full 模式下能跑标准 Docker 吗？需要什么扩展？**

需要实现以下内核扩展（见 [DESIGN.md §13](docs/DESIGN.md)）：
- **动态链接**——Docker 守护进程是动态链接的 ELF，需要内核支持 PT_INTERP + MAP_SHARED；
- **Capabilities + Seccomp**——Docker 安全模型依赖 Linux capability 位和 seccomp BPF 过滤；
- **devpts**——`docker exec` 需要 PTY（/dev/ptmx + /dev/pts/N）；
- **完整 /proc**——`docker stats` 读取 /proc/self/status 等路径；
- **veth/bridge**——Docker 默认网络模式（bridge）依赖 veth 对 + 二层转发。

**Q9：能跑 apt install 吗？**

full 模式可以。apt 依赖：
1. 动态链接（apt/dpkg 本身是动态链接二进制）；
2. ext4 磁盘文件系统（/var/lib/dpkg 持久化状态）；
3. HTTPS 下载（内核 TCP + TLS 在用户态）；
4. tar/gzip 解压（纯用户态实现）。

这些都在 §13 扩展性设计中覆盖。

**Q10：JVM 需要哪些特殊内核支持？**

JVM 是最"挑剔"的用户态程序：
- **动态链接**——libjvm.so 是 ~20MB 共享库；
- **futex**——Java synchronized 底层是 pthread mutex → futex；
- **TLS**——每个 Java 线程有独立的 Thread 对象，存在 TLS 区；
- **信号**——JVM 捕获 SIGSEGV 做 null 检查、SIGPROF 做性能采样，需要 sigaltstack + SA_SIGINFO；
- **getrandom**——SecureRandom 初始化；
- **/proc/self/maps**——JVM 读取自身内存映射做 GC 优化。

**Q11：Python 呢？比 JVM 简单吗？**

简单一些。Python 需要：动态链接（libpython3.x.so）、getrandom（os.urandom）、基本信号。Python GIL 也是 futex，但 Python 的信号使用比 JVM 更简单（不做 SIGSEGV 捕获）。
