# Novos‑OS 开发步骤（路线图）v2.0

> 本路线图基于 DESIGN.md v0.1 及 2026-08-26 深度评审（12 项勘误已内联）制定。
> **版本发布策略与关键设计决策索引见 [VERSIONING.md](VERSIONING.md)**。
> **核心目标**：从零构建一个 **≤32MB（minimal）/ ≤40MB（full）** 的微型容器宿主内核。
>
> **开发原则**：
> 1. **先跑通，再优化** —— 每个里程碑必须可运行，不追求完美。
> 2. **内存预算是红线** —— 每阶段验收测量，超标即阻断。
> 3. **并行优先** —— M4（VFS）、M5（网络）、M6（隔离） 互不依赖，可并行走。

---

## 里程碑总览（minimal 核心）

| # | 里程碑 | 核心交付 | 内存预算（估） | 前置依赖 | 建议工期 |
|---|---|---|---|---|---|
| M0 | 最小可启动内核 | 串口打印 + panic 处理 | 2–3 MB | — | 2 周 |
| M1 | 物理内存 + 内核堆 | Buddy + Slab 分配器 | 4–6 MB | M0 | 2 周 |
| M2 | 虚拟内存 + 任务调度 | 多任务切换 + 同步原语 + **SMP 占位** | 8–12 MB | M1 | 4 周 |
| M3 | 系统调用 + 用户态 | `init` + `shell` 启动 | 14–18 MB | M2 | 3 周 |
| **并行组 1** | | | | | |
| M4 | VFS + ramfs/tmpfs | 文件系统抽象 + 内存文件 | 18–22 MB | M3 | 3 周 |
| M5 | 网络栈 + epoll | TCP/IP + HTTP 服务 | 24–28 MB | M4 | 6 周 |
| M6 | Namespace + Cgroup | 进程/资源隔离 | 26–30 MB | M3 | 3 周 |
| **合并** | | | | | |
| M7 | OverlayFS | 容器镜像分层 | 28–31 MB | M4 + M6 | 3 周 |
| M8 | 容器运行时 + 网关 | `novos run` + NAT 转发 | 30–32 MB | M5+M6+M7 | 4 周 |
| M9 | 长期稳定版 | 32MB 基线正式达标 | **≤32 MB** | M8 | 4 周 |

**总计工期（minimal 1.0）**：约 **34 周**（~8.5 个月，3 人并行可压缩至 5 个月）

---

## 里程碑详细任务（minimal）

### M0：最小可启动内核 + 串口输出
**目标**：能在 QEMU 中启动，并输出日志。

- [ ] 搭建 Rust `no_std` 内核工程（`x86_64-unknown-none` 目标）。
- [ ] 编写链接脚本 `linker.ld`（`.text/.rodata/.data/.bss` 布局）。
- [ ] **引导协议**：自包含 multiboot1（QEMU 扁平镜像）/ PVH（QEMU ELF）/ multiboot2（GRUB）三协议（M0 已实现，见 kernel/boot.asm）。
- [ ] 初始化临时页表（恒等映射或偏移映射）。
- [ ] **PlatformInfo 抽象**：启动信息（内存布局/中断基址/MMIO/时钟频率）统一结构，x86 在 boot 层填充、ARM 后续用 dtb_parse() 填充（勘误③，DESIGN §1.3）。
- [ ] 8250 UART 驱动：`print!`/`println!` 宏。
- [ ] 中断/异常处理：IDT + GDT 骨架，`panic` 落串口。

**验收标准**：
- QEMU 启动后串口打印 `Novos-OS: boot ok` 与内存映射信息。
- 手动触发 panic（如 `assert_eq!(1, 2)`）能打印寄存器快照且不重启。

---

### M1：物理内存 + 内核堆
**目标**：能动态分配内存（`Box`/`Vec` 可用）。

- [ ] 解析 bootloader 的物理内存映射（E820），构建 `PageFrame` 数组。
- [ ] Buddy 分配器（order 0–10）：分配、分裂、释放、合并。
- [ ] Slab 分配器（固定 size 阶梯，**侵入式空闲链表**）。
- [ ] 接入 `GlobalAlloc`，使 `Vec`/`Arc` 可用。
- [ ] **可移动页标记**（`MIGRATE_MOVABLE`）用于后续碎片整理。

**验收标准**：
- 单元测试：buddy 分配/释放无泄漏、无重叠；slab 对象复用正确。
- 能 `Vec::with_capacity(10_000)` 并释放，内存统计 `used` 回落。

---

### M2：虚拟内存 + 任务 + 调度
**目标**：实现多任务并发切换。

- [ ] 4 级页表管理：`mmap`/`munmap`、懒分配、COW。
- [ ] `Task` 结构（§3.3）+ 内核栈 + 上下文切换汇编。
- [ ] CFS 简化调度器：vruntime 红黑树、tick 抢占（**结构预留 RT 类双队列**）。
- [ ] `fork`（克隆 Task + 页表 COW）、`exit`、`waitpid`。
- [ ] 同步原语：`Spinlock`/`Mutex`/`WaitQueue`（**内置优先级继承 PIP**）。
- [ ] **PIP × 锁层级边界**：PIP 仅解决优先级反转、不改变全局锁层级顺序；审查清单加"PIP 与锁层级冲突检测"（勘误，DESIGN §4.2）。
- [ ] **RT 任务强制 Spinlock（关抢占）+ CFS Mutex 关内核抢占**（勘误 §11，锁序闭环）。
- [ ] **SMP 预热**：引入 `per_cpu!` 宏 + `cpu_rq(cpu_id)` 访问器（即使只初始化 CPU 0）。
- [ ] 定时器最小堆 + 时钟中断（**预留时钟源抽象/monotonic/RTC**）。

**验收标准**：
- 多个内核线程轮转执行，可睡眠唤醒。
- `fork` 后父子地址空间独立（COW 生效）。
- **PIP 测试**：低优先级任务持锁，高优先级任务等待时，持锁者优先级被临时提升。

---

### M3：系统调用 + 用户态 init/shell
**目标**：能跑用户态程序，命令行可用。

- [ ] `syscall` 指令入口 + 系统调用表（`read/write/open/close/exit/…`）。
- [ ] 用户态内存管理：`mmap`/`munmap`/`brk`。
- [ ] ELF 加载器（静态链接 musl 二进制）。
- [ ] `init`（PID 1）+ 简易 `shell`（`fork`+`exec`+管道）。
- [ ] **PID 1 崩溃自愈**：内核检测到 PID 1 退出时，尝试执行 `rescue_init`（.rodata 内嵌），带**60 秒反跳计时器**（3 次崩溃后强制 Watchdog 复位）。
- [ ] 文件描述符表、`/dev/uart` 设备文件。

**验收标准**：
- `make run` 后进入 shell，能执行 `ls`/`cat`/`echo` 等命令。
- **首次内存基线**：空闲内核常驻 ≤ 18MB（作为对照锚点）。

---

### M4：VFS + ramfs/tmpfs（与 M5/M6 并行）
**目标**：完整文件系统抽象。

- [ ] VFS 层：`SuperBlock/Inode/Dentry/File`（§3.6）。
- [ ] dcache：hash 查找 + LRU 可回收 + shrink_target。
- [ ] ramfs（initramfs 挂载） + tmpfs（页缓存文件）。
- [ ] 系统调用：`open/read/write/close/stat/mkdir/rmdir/unlink/readdir/mount`。
- [ ] 路径解析（§4.3）+ 挂载点遍历。

**验收标准**：
- 在 tmpfs 上完整读写/建目录/枚举目录。
- 删除文件后内存回落（回收路径生效）。

---

### M5：网络栈 + socket + epoll（与 M4/M6 并行）
**目标**：TCP/IP 完整栈，能跑 HTTP。

- [ ] 以太网 + virtio‑net 驱动；ARP。
- [ ] IPv4：收发、分片最小化、ICMP echo。
- [ ] UDP socket + TCP（三次握手、滑动窗口、重传、NewReno）。
- [ ] **Skb 内存池（评估）**：DMA 池 + Arc 引用计数零拷贝接收路径（池上限 256KB）。
- [ ] **TCP 已确认段批量回收**：ACK 后 `snd_una` 前移，批量释放 retrans_queue 已确认 Skb（Arc 归零自动还池，DESIGN §4.5 新增）。
- [ ] `socket/bind/listen/accept/connect/send/recv`。
- [ ] `epoll_create/epoll_ctl/epoll_wait`（LT+ET）。
- [ ] **SNTP 客户端**（UDP 123，工业时钟校准）。

**验收标准**：
- 内核内起 HTTP 服务，QEMU 内 `wget` 成功。
- 并发 100 个连接，epoll 正确唤醒、无丢包。

---

### M6：Namespace + Cgroup（与 M4/M5 并行）
**目标**：进程/资源隔离能力。

- [ ] 7 种 namespace：pid/mnt/net/uts/ipc/user/cgroup。
- [ ] `clone` 带 flags → 创建 namespace。
- [ ] pid namespace：ns 内 pid=1，跨 ns 可见性。
- [ ] Cgroup v2 树 + memory/pids 控制器。
- [ ] `memory.max` 触发回收 / OOM‑kill（仅杀容器内进程）。

**验收标准**：
- `clone` 新进程在不同 pid/uts/net ns，隔离生效。
- cgroup 限制 `memory.max=64MB`，超限进程被 kill，宿主无恙。

---

### M7：OverlayFS
**目标**：镜像层 + 可写层的文件系统。

- [ ] `mount -t overlay`：lower/upper/work。
- [ ] 合并 lookup（§4.4）：upper→lower 逐层，whiteout 处理。
- [ ] **稀疏 copy-up**（extent-based，只复制被修改块，防 OOM）。
- [ ] **容器日志目录默认 tmpfs**，禁止持久化日志触发 copy-up。

**验收标准**：
- 只读 lower 上叠加可写 upper，写后下层不变。
- 删除下层文件产生 whiteout，重启后仍“已删除”。

---

### M8：容器运行时 + 网关
**目标**：跑起第一个容器 + 网关转发。

- [ ] 类 runC 命令：`novos run <image> <cmd>`（§4.6 完整流程）。
- [ ] 容器 rootfs 用 OverlayFS 组装；`pivot_root`。
- [ ] 容器内挂 `/proc`（pid ns 视图）、设 cgroup。
- [ ] 网关：IP 转发 + conntrack + MASQUERADE + 端口映射。
- [ ] 基础防火墙（线性规则表）。

**验收标准**：
- `novos run busybox echo hello` 输出 hello，容器隔离正确。
- 容器外网访问经 MASQUERADE 成功，conntrack 老化回收。
- **内存基线**：空闲 32MB 内；跑 3 个容器内核常驻不涨破预算。

---

### M9：长期稳定版（压测 / 收敛 / 回填）
**目标**：32MB 基线正式达标，生产可用。

- [ ] **内存紧缩（compact_zone）**：order ≥ 3 分配失败时触发低阶可移动页合并。
- [ ] **分层时间轮**：替换最小堆，支持 1000+ TCP 连接定时器 O(1) 维护。
- [ ] 内存回归测试套件：启动 N 容器 + 网关流量下断言常驻 ≤32MB。
- [ ] 每个子系统 shrink 路径压测（dcache/sk_buff/anon）。
- [ ] 编译期瘦身终检：`opt-level=s/z` + LTO + strip，回填实测 .text 大小。
- [ ] 稳定性：长跑 7 天无泄漏（used 不单调爬升）。
- [ ] 看门狗 + 掉电保护（日志原子写 / FS 一致性）。
- [ ] 可观测性：环形日志 + 落盘 + 健康指标（内存/fd/CPU）。

**验收标准**：
- 文档 2.1/2.2 的每一行预算都有**实测数据支撑**。
- CI 中内存断言通过（`kernel_used ≤ 32MB`）。

---

## 扩展里程碑（full 模式：Linux 兼容容器宿主）

> **注意**：以下 M10–M14 在 minimal 版稳定后推进，通过 Cargo `--features full` 编译。
> 内存预算调整为 **≤ 40MB**（DESIGN.md §5.3 预算台账口径）。

### M10：Block I/O + ext4 + Page Cache
**目标**：支持磁盘文件系统。

- [ ] `BlockDevice` trait + virtio-blk 驱动。
- [ ] BIO 层（简单队列 + 同步 I/O）。
- [ ] Page Cache（`AddressSpace`：文件偏移 → 物理页，可 shrink）。
- [ ] ext4 驱动：`data=journal` 完整模式（防掉电损坏）。
- [ ] `mmap MAP_SHARED` 文件映射（多进程共享物理页）。

**验收标准**：
- ext4 上创建/读写/删除文件，重启后持久化。
- 两个进程 mmap 同一 .so 文件 → 共享同一物理页。

---

### M11：动态链接 + futex + TLS + 工具链
**目标**：能跑动态链接的 ELF + pthread 线程。

- [ ] ELF 加载器扩展：识别 `PT_INTERP` + 设置辅助向量（AT_BASE/AT_RANDOM）。
- [ ] 加载 `ld-musl-x86_64.so.1` 到地址空间。
- [ ] futex 系统调用（WAIT/WAKE/REQUEUE，**逻辑键索引**，支持 COW 迁移）。
- [ ] TLS：`arch_prctl(ARCH_SET_FS)` + FS base MSR。
- [ ] **宿主机交叉编译工具链**：musl-cross + `crt1.o` + linker script。
- [ ] **`novos-check` 工具**：扫描 ELF 的 syscall 依赖 + 内存足迹预估（M14 应用合入门槛）。

**验收标准**：
- 运行动态链接的 hello world（`gcc -o hello hello.c` 不加 `-static`）。
- `pthread_create` 创建线程 + futex 互斥锁正常工作。

---

### M12：设备框架 + Capabilities + Seccomp
**目标**：Docker 安全模型 + 完整设备文件。

- [ ] **驱动框架定型**：bus→device→driver + BSP + 中断分发。
- [ ] 标准设备：`/dev/null`、`/dev/zero`、`/dev/urandom`、`/dev/ptmx`。
- [ ] Capabilities：`TaskCreds`（permitted/effective/inheritable/bounding）。
- [ ] Seccomp BPF：参数值匹配（eq/ne/masked_eq，覆盖 mount/ptrace/openat 等 10 个高风险调用）。

**验收标准**：
- `echo hello > /dev/null` + `dd if=/dev/zero bs=4096 count=1`。
- seccomp profile 禁止 `reboot` syscall → 容器内调 reboot 返回 EPERM。

---

### M13：完整 /proc + 信号扩展 + 事件 fd
**目标**：动态链接程序可观测性 + 完整信号。

- [ ] /proc 扩展：`/proc/self/maps`、`/proc/self/status`、`/proc/self/exe`、`/proc/self/fd/`。
- [ ] 信号扩展：`sigaction`（SA_SIGINFO + SA_ONSTACK）、`sigprocmask`、`sigaltstack`。
- [ ] timerfd：`timerfd_create` + `timerfd_settime` + epoll 可监听。
- [ ] **Block I/O 电梯调度（评估）**：READ 优先 + 写 LBA 合并的极简 Deadline（勘误②，DESIGN §13.3，~300 行）。

**验收标准**：
- 动态链接 busybox 启动后 `/proc/self/maps` 输出正确的地址空间布局。
- 动态程序捕获 SIGSEGV 正常（`sigaltstack` + SA_SIGINFO）。

---

### M14：OCI 镜像 + 轻量容器运行时（OTA 升级回滚）
**目标**：full 模式正式达标 —— `novos-pull` + Redis/SQLite 容器服务 + OTA 演示。

- [ ] veth pair + bridge 虚拟网络设备 + 完整 DNAT 端口映射。
- [ ] OCI runtime spec 兼容（config.json 解析 + seccomp/capability 应用）。
- [ ] `novos-pull`：registry HTTPS + OCI 解析 + SHA-256 摘要校验 + 层解压。
- [ ] **OTA 升级 + 回滚**：增量拉取变化层 + 镜像版本切换（出错切回旧层）。
- [ ] **最小记录锁**：`fcntl(F_SETLK/F_GETLK/F_UNLCK)` 字节区间锁（SQLite 依赖）。
- [ ] 移植 **SQLite**（musl 静态，CRUD + WAL） + **Redis**（**部署模板强制**：`--maxmemory 64mb`、禁 RDB、只开 AOF）。
- [ ] （值得）内置 Web 管理界面 + SSH（dropbear）+ Agent 主动上联。

**验收标准**：
- `novos run redis` 容器启动，外部 `redis-cli SET/GET` 通过端口映射可访问。
- **OTA 演示**：更新镜像层 → 增量拉取 → 重启生效；回滚旧层可恢复。
- **内存基线**：≤40MB（full 模式最终断言）。

---

## 开发节奏与并行策略

| 时间段 | 核心任务 | 并行任务 |
|---|---|---|
| **第 1–4 周** | M0 + M1（基础内核） | 无 |
| **第 5–8 周** | M2（虚拟内存 + 调度） | 无 |
| **第 9–11 周** | M3（用户态启动） | 无 |
| **第 12–17 周** | **M4（VFS）** + **M5（网络栈）** + **M6（隔离）** 三路并行 | 核心团队 3 人 |
| **第 18–20 周** | M7（OverlayFS） | 依赖 M4+M6 |
| **第 21–24 周** | M8（容器+网关） | 依赖 M5+M6+M7 |
| **第 25–28 周** | M9（稳定版 + 内存收敛） | 全团队回归测试 |
| **第 29 周+** | M10–M14（full 模式） | 按需并行 |

---

## 测试与验收总表

| 里程碑 | 单元测试（host） | 集成测试（QEMU） | 内存断言 |
|---|---|---|---|
| M0 | 无 | 串口输出 `boot ok` + panic 快照 | 仅记录 .text |
| M1 | buddy/slab 分配/释放 | `Vec::push` 不 panic | free/used 页数 |
| M2 | rbtree 插入/最左；PIP 锁提升 | 多内核线程轮转 + fork COW | — |
| M3 | syscall 参数解析；ELF 段加载 | shell 跑 `ls`/`echo` | **≤18MB** |
| M4 | 路径解析；dcache 命中/miss | tmpfs 读写 + 删除回落 | dcache shrink |
| M5 | TCP 状态机转换；epoll 就绪 | HTTP + 100 连并发 | sk_buff 上限 |
| M6 | pid ns 层级；cgroup 记账 | clone 隔离 + OOM kill | cgroup 对象计数 |
| M7 | overlay lookup；copy-up 流程 | 写后下层不变 + whiteout | overlay cache |
| M8 | 容器全流程；NAT 规则 | `novos run busybox` + 外网 | **3 容器≤32MB** |
| M9 | 全回归 | 7 天长跑 + 1000 连接 | **≤32MB 最终** |
| M10–M14 | 各模块单元测试 | 对应场景（ext4/docker/apt/jvm） | **≤40MB** |

---

## 风险与缓解策略

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| **TCP 栈复杂度超预期** | 高 | M5 延期 | 先 NewReno，Cubic 留 M9；状态机单元测试先行 |
| **32MB 预算超标** | 中 | M9 无法达标 | 从 M3 起每阶段测量基线，超标即排查 |
| **OverlayFS copy-up 正确性** | 中 | 容器写数据丢失 | 边界测试：并发写/大文件/中断恢复 |
| **Rust `no_std` 生态缺口** | 低 | 需自实现部分模块 | 网络/TCP 自实现，基础 crate（spin/bitflags）已成熟 |
| **多核 SMP 后期重构** | 中 | 架构返工 | **M2 已预留 per-CPU 占位**，降低重构风险 |

---

## CI/CD 流水线

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
      - name: fmt & clippy
        run: cargo fmt --all -- --check && cargo clippy -- -D warnings
      - name: unit tests
        run: cargo test --workspace
      - name: QEMU integration
        run: make test-integration
      - name: Memory regression
        run: make test-memory