# 山水观心操作系统开发步骤（路线图）v2.0

> 本路线图基于 DESIGN.md v0.1 及 2026-08-26 深度评审（12 项勘误已内联）制定。
> **版本发布策略与关键设计决策索引见 [VERSIONING.md](VERSIONING.md)**。
> **核心目标**：从零构建一个 **≤32MB（minimal）/ ≤40MB（full）** 的微型容器宿主内核（山水观心 Core）。
> **图形版 Desktop（≥128MB，含 GPU/GUI）为独立产品线**，见 M16（feature-gated，默认不编译）。
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
| M8 | 容器运行时 + 网关 | `shanshui-guanxin run` + NAT 转发 | 30–32 MB | M5+M6+M7 | 4 周 |
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
- [ ] 内核命令行解析：支持 `log=debug`、`console=ttyS0` 等参数。

**验收标准**：
- QEMU 启动后串口打印 `Shanshui-guanxin: boot ok` 与内存映射信息。
- 手动触发 panic（如 `assert_eq!(1, 2)`）能打印寄存器快照且不重启。

---

### M1：物理内存 + 内核堆
**目标**：能动态分配内存（`Box`/`Vec` 可用）。

- [ ] 解析 bootloader 的物理内存映射（E820），构建 `PageFrame` 数组。
- [ ] Buddy 分配器（order 0–10）：分配、分裂、释放、合并。
- [ ] Slab 分配器（固定 size 阶梯，**侵入式空闲链表**）。
- [ ] 接入 `GlobalAlloc`，使 `Vec`/`Arc` 可用。
- [ ] **可移动页标记**（`MIGRATE_MOVABLE`）用于后续碎片整理。
- [ ] 内存统计初始化：`MemStat` 结构，记录 `total_pages`、`free_pages`、`slab_used`。

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
- [ ] `/etc/motd` 登录提示："本设备不包含编译器，请使用宿主机交叉编译（docs/nosos-sdk.md）"；`PATH` 不放置 go/rustc/g++（DESIGN §21.4）。
- [ ] **PID 1 崩溃自愈**：内核检测到 PID 1 退出时，尝试执行 `rescue_init`（.rodata 内嵌），带**60 秒反跳计时器**（3 次崩溃后强制 Watchdog 复位）。
- [ ] 文件描述符表（**`Vec<Option<Arc<File>>>` + 低 64 位空闲位图**，评审定案：fd 稠密整数数组 O(1)，非 BTreeMap）、`/dev/uart` 设备文件。
- [ ] 首次内存基线记录：`docs/bench/m3-baseline.txt` 记录 `.text` 大小 + 空闲 `used`。

**验收标准**：
- `make run` 后进入 shell，能执行 `ls`/`cat`/`echo` 等命令。
- **首次内存基线**：空闲内核常驻 ≤ 18MB（作为对照锚点）。

---

### M4：VFS + ramfs/tmpfs（与 M5/M6 并行）
**目标**：完整文件系统抽象。

- [ ] VFS 层：`SuperBlock/Inode/Dentry/File`（§3.6）。
- [ ] dcache：hash 查找 + LRU 可回收 + shrink_target（哈希键用 **FNV-1a**——热路径哈希表禁 SipHash，见 DESIGN §3.6）。
- [ ] **dcache shrink 阈值**：`entries > shrink_target` 时触发回收，目标 `shrink_target * 0.8`；`entries > shrink_watermark` 时强制立即回收（DESIGN §3.6 新增）。
- [ ] ramfs（initramfs 挂载） + tmpfs（页缓存文件）。
- [ ] 系统调用：`open/read/write/close/stat/mkdir/rmdir/unlink/readdir/mount`。
- [ ] 路径解析（§4.3）+ 挂载点遍历。

**验收标准**：
- 在 tmpfs 上完整读写/建目录/枚举目录。
- 删除文件后内存回落（回收路径生效）。
- **dcache shrink 测试**：创建 1000 个文件，读取后触发 shrink，entries 回落。

---

### M5：网络栈 + socket + epoll（与 M4/M6 并行）
**目标**：TCP/IP 完整栈，能跑 HTTP。

- [ ] 以太网 + virtio‑net 驱动；ARP。
- [ ] IPv4：收发、分片最小化、ICMP echo。
- [ ] UDP socket + TCP（三次握手、滑动窗口、重传、NewReno）。
- [ ] **Skb 内存池（评估）**：DMA 池 + Arc 引用计数零拷贝接收路径（池上限 256KB）。
- [ ] **TCP 已确认段批量回收**：ACK 后 `snd_una` 前移，批量释放 retrans_queue 已确认 Skb（Arc 归零自动还池，DESIGN §4.5 新增）。
- [ ] `socket/bind/listen/accept/connect/send/recv`。
- [ ] `epoll_create/epoll_ctl/epoll_wait`（LT+ET；`HashMap<fd, EpollItem>` + `VecDeque` 就绪队列，**单实例 fd > 1024 且高频增删才升红黑树**，见 DESIGN §3.8）。
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
- [ ] **Cgroup 记账测试**：`page_charge`/`page_uncharge` 配对，无泄漏。

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

- [ ] 类 runC 命令：`shanshui-guanxin run <image> <cmd>`（§4.6 完整流程）。
- [ ] 容器 rootfs 用 OverlayFS 组装；`pivot_root`。
- [ ] 容器内挂 `/proc`（pid ns 视图）、设 cgroup。
- [ ] 网关：IP 转发 + conntrack + MASQUERADE + 端口映射。
- [ ] **conntrack 老化策略**：`ESTABLISHED` 120s、`NEW` 30s、`ICMP` 10s；到期自动删除（DESIGN §3.8 新增）。
- [ ] 基础防火墙（线性规则表）。

**验收标准**：
- `shanshui-guanxin run busybox echo hello` 输出 hello，容器隔离正确。
- 容器外网访问经 MASQUERADE 成功，conntrack 老化回收。
- **内存基线**：空闲 32MB 内；跑 3 个容器内核常驻不涨破预算。

---

### M9：长期稳定版（压测 / 收敛 / 回填）
**目标**：32MB 基线正式达标，生产可用。

- [ ] **内存紧缩（compact_zone）**：order ≥ 3 分配失败时触发低阶可移动页合并。
- [ ] **分层时间轮**：替换最小堆，支持 1000+ TCP 连接定时器 O(1) 维护。
- [ ] **RT 调度类（SCHED_FIFO 基本模型）**：优先级 + 抢占，从 M2 RT 双队列预留固化（Modbus 等硬实时场景 100ms 响应，DESIGN §21.7）。
- [ ] **/proc/cpuinfo 多核报告**：报告硬件真实核心数，`online` 只显示 1（SMP 前避免用户误判板子坏了，DESIGN §21.5）。
- [ ] 内存回归测试套件：启动 N 容器 + 网关流量下断言常驻 ≤32MB。
- [ ] 每个子系统 shrink 路径压测（dcache/sk_buff/anon）。
- [ ] 编译期瘦身终检：`opt-level=s/z` + LTO + strip，回填实测 .text 大小。
- [ ] 稳定性：长跑 7 天无泄漏（used 不单调爬升）。
- [ ] 看门狗 + 掉电保护（日志原子写 / FS 一致性）。
- [ ] **环形日志落盘**：内核日志异步写 `/var/log/kernel.log`，内存缓冲 → 批量写 Ext4（§10.1→§19.2 衔接）。
- [ ] **健康指标暴露**：`/proc/health` 输出 JSON：内存 used/free、fd 数、容器数、CPU 负载。
- [ ] **top 数据源（/proc 扩展）**：
  - [ ] **`/proc/stat`**：CPU 总时间 / 各态时间、中断计数、上下文切换次数（DESIGN §10.2）；
  - [ ] **`/proc/loadavg`**：1/5/15 分钟负载平均值；
  - [ ] **`/proc/<pid>/stat`**：每进程 pid、状态、utime/stime、rss、vsize。
- [ ] **编写 top 命令**（Rust，静态编译，<100KB，`--features top`）：
  - 循环读取 `/proc/stat`、`/proc/loadavg`、`/proc/<pid>/stat`，计算进程 CPU 占用率；
  - 交互式界面（crossterm 或简单 ANSI 转义），刷新间隔 1s；
  - 支持按 CPU/内存排序、按键 `q` 退出、`h` 帮助。
- [ ] 将 top 放入默认 rootfs（`/bin/top`），在 M15 用户文档中说明使用方法。
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
- [ ] BIO 层（简单队列 + 同步 I/O + **错误重试：读失败返回 EIO，写失败重试 3 次（间隔 10ms），超时 5s**）。
- [ ] **电梯调度（Deadline，可选）**：READ 优先 + 写 LBA 合并的极简 Deadline（`--features block-scheduler`，对应 M13 评估项）。
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
- [ ] **`shanshui-guanxin-check` 工具**：扫描 ELF 的 syscall 依赖 + 内存足迹预估（RSS+虚拟内存），不通过禁止合入；**启动前扫描 `PT_INTERP`，非 `/shanshui-guanxin/ld-musl` 拒绝启动并提示**（glibc 拦截，DESIGN §21.1）。
- [ ] **动态链接依赖检查**：`shanshui-guanxin-check` 解析 `DT_NEEDED` 依赖链，确认 ld.so 均位于 `/shanshui-guanxin/` 下（`/shanshui-guanxin/ld-musl-x86_64.so.1` 路径校验）。
- [ ] **官方推荐软件清单（阶段一）**：文档页表格列出软件/功能/官方地址/官方验证 musl 静态二进制链接（core/runtime/service/net-tools 四类，DESIGN §22）；配套 `shanshui-guanxin-build` 一键构建；**立即产出《为 山水观心操作系统构建 Redis》musl 静态编译指南**（Redis 7.2.4+）。

**验收标准**：
- 运行动态链接的 hello world（`gcc -o hello hello.c` 不加 `-static`）。
- `pthread_create` 创建线程 + futex 互斥锁正常工作。

---

### M12：设备框架 + Capabilities + Seccomp
**目标**：Docker 安全模型 + 完整设备文件。

- [ ] **驱动框架定型**：bus→device→driver + BSP + 中断分发。
- [ ] **设备类型增加 GPU（BusType::Gpu）**：为 M16 图形版预留接口（`feature = "gpu"`，DESIGN §6.2⑤/§13.16）。
- [ ] 标准设备：`/dev/null`、`/dev/zero`、`/dev/urandom`、`/dev/ptmx`。
- [ ] Capabilities：`TaskCreds`（permitted/effective/inheritable/bounding）。
- [ ] Seccomp BPF：参数值匹配（eq/ne/masked_eq，覆盖 mount/ptrace/openat 等 10 个高风险调用）。

**验收标准**：
- `echo hello > /dev/null` + `dd if=/dev/zero bs=4096 count=1`。
- seccomp profile 禁止 `reboot` syscall → 容器内调 reboot 返回 EPERM。

---

### M13：完整 /proc + 信号扩展 + 事件 fd
**目标**：动态链接程序可观测性 + 完整信号。

- [ ] /proc 扩展：`/proc/self/maps`、`/proc/self/status`、`/proc/self/exe`、`/proc/self/fd/`、`/proc/net/conntrack`（纯文本：协议/剩余秒/状态/五元组，DESIGN §10.2 新增）。
- [ ] 信号扩展：`sigaction`（SA_SIGINFO + SA_ONSTACK）、`sigprocmask`、`sigaltstack`。
- [ ] timerfd：`timerfd_create` + `timerfd_settime` + epoll 可监听。
- [ ] **网络调试开关**：`/proc/sys/net/shanshui-guanxin/packet_trace`——开启后环形日志打印每包五元组 + 丢弃原因（性能降 ~50%，替代 tcpdump，DESIGN §21.8）。
- [ ] **Block I/O 电梯调度（评估）**：READ 优先 + 写 LBA 合并的极简 Deadline（勘误②，DESIGN §13.3，~300 行）。

**验收标准**：
- 动态链接 busybox 启动后 `/proc/self/maps` 输出正确的地址空间布局。
- 动态程序捕获 SIGSEGV 正常（`sigaltstack` + SA_SIGINFO）。

---

### M14：OCI 镜像 + 轻量容器运行时（OTA 升级回滚）
**目标**：full 模式正式达标 —— `shanshui-guanxin-pull` + Redis/SQLite 容器服务 + OTA 演示。

- [ ] veth pair + bridge 虚拟网络设备 + 完整 DNAT 端口映射。
- [ ] OCI runtime spec 兼容（config.json 解析 + seccomp/capability 应用）。
- [ ] `shanshui-guanxin-pull`：registry HTTPS + OCI 解析 + SHA-256 摘要校验 + 层解压。
- [ ] **OTA 升级 + 回滚**：增量拉取变化层 + 镜像版本切换（出错切回旧层）；**内核镜像纳入 A/B 分区管理**（内核分区 A/B 标识 + 回滚，覆盖内核本身升级，DESIGN §21.9）。
- [ ] **Redis 部署模板**：预置只读 `/etc/redis/redis.conf`（Immutable），`--maxmemory 64mb`、禁 RDB、只开 AOF——用户无法 `-c` 覆盖导致 OOM（DESIGN §21.2）。
- [x] **最小记录锁**：`fcntl(F_SETLK/F_GETLK/F_UNLCK)` 字节区间锁（SQLite 依赖）。
- [ ] 移植 **SQLite**（musl 静态，CRUD + WAL） + **Redis**（**部署模板强制**：`--maxmemory 64mb`、禁 RDB、只开 AOF）。
- [ ] **实现 shanshui-guanxin-gateway**（`--features gateway`，Rust 静态编译，纯用户态）：
  - [ ] **配置格式**（TOML）：监听端口、上游服务、路由规则、TLS 证书路径（示例 `/etc/shanshui-guanxin/gateway.toml`）；
  - [ ] **HTTP/1.1 服务**（基于 M5 TCP/socket）+ **TLS（rustls）**（80/443）；
  - [ ] **反向代理核心**：`ProxyPass` 转发到容器后端，注入 `X-Forwarded-For`，keep-alive 连接池；
  - [ ] **WebSocket 升级**（用于 Web 管理界面的实时日志）；
  - [ ] **系统服务集成**：`shanshui-guanxin gateway start/stop/status`，支持守护进程（fork/daemonize）；
  - [ ] **默认配置**：监听 80/443，`/api/` 转发容器后端，静态文件服务 `/ui/`（Web 管理界面）。
- [ ] （值得）内置 Web 管理界面 + SSH（dropbear）+ Agent 主动上联。

**验收标准**：
- `shanshui-guanxin run redis` 容器启动，外部 `redis-cli SET/GET` 通过端口映射可访问。
- **OTA 演示**：更新镜像层 → 增量拉取 → 重启生效；回滚旧层可恢复。
- **内存基线**：≤40MB（full 模式最终断言）。

---

### M15：发布准备与用户支持体系（v1.0 发布前）

**目标**：构建完整的用户入门与支持体系，确保第一批用户能在 30 分钟内从零开始运行第一个容器（DESIGN §24）。

- [ ] **预构建环境**：
  - 生成 QEMU 镜像（`make qemu-image`）并上传至官方下载站点；
  - 编写一键启动脚本 `shanshui-guanxin-run.sh`，支持 QEMU 参数自动适配；
  - 测试镜像在至少两种 QEMU 配置（`-machine pc` 和 `-machine virt`）下可启动。
- [ ] **核心文档**：
  - 编写 `docs/quickstart.md`（5 分钟快速开始）；
  - 编写 `docs/first-container.md`（运行 busybox 容器）；
  - 编写 `docs/migration-guide.md`（从 Linux 迁移避坑指南）。
- [ ] **开发者工具链**：
  - 提供交叉编译 SDK 的 Dockerfile（`docker/Dockerfile.sdk`），包含 musl-cross、crt1 等；
  - 提供示例应用仓库 `shanshui-guanxin-examples`，包含 C/Rust 的 Hello World 及 `Makefile`/`build.rs`；
  - 确保 `shanshui-guanxin-build` 命令已集成到 `shanshui-guanxin` CLI（或作为独立脚本），并完成 `shanshui-guanxin-check` 集成。
- [ ] **调试与反馈**：
  - 配置串口日志输出（默认开启，可收集）；
  - 在 GitHub 创建 Issue 模板（`.github/ISSUE_TEMPLATE/bug_report.md`），引导用户填写必要信息；
  - 创建社区沟通群组（Discord 或 Telegram），并在 README 中公布加入链接。
- [ ] **路线图与更新机制**：
  - 在 README 中增加 `ROADMAP.md` 链接，展示当前版本与下一版本目标；
  - 建立社区更新发布流程（如每两周发布一次 `CHANGELOG.md` 更新）。

**验收标准**：
- 一位从未接触过 山水观心操作系统的开发者，按照 `docs/quickstart.md` 的指引，能在 15 分钟内下载镜像并启动 shell。
- 该开发者按照 `docs/first-container.md`，能在 5 分钟内成功运行 `shanshui-guanxin run busybox echo hello` 并看到输出。
- 该开发者能使用 SDK 在宿主机上交叉编译示例程序，并部署到 QEMU 中运行。
- 该开发者在遇到问题时，能通过 Issue 模板在 10 分钟内提交一份包含完整日志的报告。
- 社区沟通渠道至少有一名核心开发者在 24 小时内响应。

**内存影响**：无（均为用户态工具和文档，不计入内核预算）。

---

### M16：图形版 Desktop（独立产品线，`--features full,gui`）

> 与 Core（M0–M15）**独立核算**：图形版内存目标 **≥128MB**（DESIGN §2.4），不适用 32MB/40MB 断言。
> 阶段规划见 DESIGN_EXTENSION §4.1（主线四），内核侧最小显示子集见 DESIGN §13.16。

#### M16-1：帧缓冲设备（`/dev/fb*`，`feature = "framebuffer"`）

- [ ] 实现 **fbdev 框架**：注册帧缓冲设备，提供 `open/read/write/mmap/ioctl` 接口；
- [ ] 支持 **FBIOGET_VSCREENINFO / FBIOPUT_VSCREENINFO**：查询/设置分辨率与色深；
- [ ] QEMU `-vga virtio` 下初始化 **virtio-gpu**（或 bochs-display）设备；
- [ ] 分配**线性帧缓冲内存**（DMA 区），暴露给用户态 `mmap`；
- [ ] 命令行冒烟：`dd if=/dev/zero of=/dev/fb0 bs=1024 count=1024` 清屏；
- [ ] `/dev/tty0` 虚拟终端（`console=tty0`），与 UART 输出并存。

**验收**：QEMU 下 `/dev/fb0` mmap 写像素可见；`FBIOGET_VSCREENINFO` 返回正确分辨率。

#### M16-2：DRM KMS 最小子集（`feature = "drm"`）

- [ ] **drm 核心**：设备注册、`drm_open`、`drm_ioctl` 分发；
- [ ] **KMS 接口**：`drm_mode_set_crtc`、`drm_mode_page_flip`、VBlank 中断处理；
- [ ] virtio-gpu 3D 加速（可选，可先只做 2D scanout）；
- [ ] 测试程序：设置分辨率、执行 page flip，观察画面无撕裂。

**验收**：模式切换 + 页翻转无撕裂；VBlank 中断计数正确（DESIGN §6.2⑤ GPU trait）。

#### M16-3：用户态 Wayland 合成器（轻量）

- [ ] 移植或编写**简化版 Wayland 合成器**（weston 精简配置或自定义轻量实现）；
- [ ] 实现 `wl_display` / `wl_compositor` 接口，支持创建 `wl_surface` 与 `wl_shell`；
- [ ] 在帧缓冲上渲染**两个窗口**（终端 + 系统监视器），支持拖动与调整大小（最小化）。

**验收**：两个窗口叠加渲染到帧缓冲，窗口可拖动/缩放。

#### M16-4：图形库 + 系统监视器 GUI

- [ ] 图形库选型：推荐 **egui 或 fltk-rs**（轻量）；
- [ ] 实现 **shanshui-guanxin-monitor**：CPU 曲线、内存柱状图、进程列表；
- [ ] 与 top **共享数据源**（读 `/proc/stat`、`/proc/loadavg`、`/proc/<pid>/stat`），可视化实时刷新。

**验收**：实时显示 CPU/内存曲线与进程列表（数据与 top 一致）。

#### M16-5：桌面应用 – 文件管理器（shanshui-guanxin-fm）+ 盘符视图

- [ ] **shanshui-guanxin-fm**（用户态）：
  - 左侧固定显示 **"文档/下载/桌面"** 三个目录（用户家目录下创建）；
  - 右侧显示当前目录文件列表（图标 + 名称 + 大小 + 修改时间）；
  - 单击进入子目录，双击打开文件（调用默认程序）；
  - 地址栏显示路径，支持输入路径跳转；
- [ ] **盘符模拟（C:/ D:）**：绑定挂载 `/mnt/c` → `/C`，在文件管理器界面映射为 `C:` 标签（用户态，DESIGN §3.6）；
- [ ] 文件**复制/移动/删除**（经系统调用）。

**验收**：文件管理器浏览/复制/删除可用；盘符视图（C:/D:）与"文档/下载/桌面"快捷入口可用。

**内存断言**：`--features full,gui` 构建后 `kernel_used ≤ 128MB`（DESIGN §5.3、§10.3），
与 minimal/full 断言互不混用。

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

> 单元测试（host `cargo test --lib`）与启动/集成自测的**覆盖范围与用例矩阵**见
> [docs/TEST-COVERAGE.md](docs/TEST-COVERAGE.md)（status_body 18 用例 + rbtree 8 用例）。

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
| M8 | 容器全流程；NAT 规则 | `shanshui-guanxin run busybox` + 外网 | **3 容器≤32MB** |
| M9 | 全回归 + top 输出核对 | 7 天长跑 + 1000 连接 | **≤32MB 最终** |
| M10–M14 | 各模块单元测试 | 对应场景（ext4/docker/apt/jvm）；shanshui-guanxin-gateway 反向代理 | **≤40MB** |
| M16（Desktop） | GPU trait 单测 | QEMU `-vga virtio` 帧缓冲/页翻转/合成器截图断言 | **≤128MB（独立）** |

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
```

**跳过策略**：纯文档变更 → 跳过 QEMU 和内存测试（提交标记 `[ci skip-build]`）。

---

## 用户需求演进路线（v1.1–v2.0+，只增不减）

> 基于三大用户画像的高频需求，按紧迫度分三档，设计依据见 [DESIGN §23](DESIGN.md)。
> 原则：内存优先（32MB/40MB 预算内）、默认禁用、模块化（feature flag 裁剪）。
> 这些任务在 v1.0（M0–M14）稳定交付后按反馈优先级择机启动。

### 近期规划（v1.1）

| 任务 | 方案 | 验收 |
|---|---|---|
| 容器保活策略 | OCI `restartPolicy`（`always`/`on-failure`/`unless-stopped`），掉电自启无人值守 | 掉电重启后容器自动拉起 |
| Web 管理界面默认开启 | `shanshui-guanxin-webui`（端口 80）：列表/启停/日志滚动/资源曲线 | 浏览器按钮化操作容器 |

### 近期规划（v1.5）

| 任务 | 方案 | 验收 |
|---|---|---|
| 4G/5G 蜂窝上网 | PPP 栈 + USB 串口驱动 + wpa_supplicant 轻量移植 | 蜂窝/Wi-Fi 模块联网成功 |
| 持久化日志 | 内核/容器日志异步写 `/var/log/journal/` + 按大小轮转 | 重启后日志可回溯 |
| WireGuard VPN | `shanshui-guanxin-vpn` 站点到站点 | 设备主动连云端 VPN |
| 存储卷独占锁 | `shanshui-guanxin run --volume-exclusive`（flock/leases） | SQLite 并发写安全 |
| MicroPython | <256KB 引擎，官方镜像入仓库 | 跑通 JSON→CSV 脚本 |
| PTP/NTP 同步 | chrony/ntpd 轻量版 + PTP 评估 | 多设备时间戳一致 |

### 远期展望（v2.0+）

| 任务 | 方案 | 验收 |
|---|---|---|
| 四层负载均衡（L4LB） | IPVS 轮询 | 多容器入口流量分发 |
| 流量镜像 | 网关镜像端口 | 灰度验证测试容器 |