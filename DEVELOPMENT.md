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

## 开发节奏建议

- 每个里程碑独立可运行，**先跑通再优化**（YAGNI）；
- 内存预算只在**里程碑验收**时测量，开发中不打断主线；
- 建议维护 `docs/bench/` 记录每次基线测量（kernel .text / 空闲 used / 峰值）；
- 里程碑间可并行：M6 不依赖 M4/M5，可与 M4/M5 并行推进。

## 测试策略

- 单元测试：buddy/slab/rbtree/路径解析/TCP 状态机（`cargo test` 在 host 跑，逻辑与硬件解耦）；
- 集成测试：QEMU 启动断言（串口日志 + 内存统计）；
- 回归测试：CI 中 M9 内存断言。
