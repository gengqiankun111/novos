# 山水观心操作系统功能说明书

> **定位**：面向内存受限设备（256MB–2GB）的微型容器宿主 —— RTOS 的占地，Linux 的生态，Rust 的安全。
> **本文件**：功能/特性清单说明书（"支持什么"），状态随实现推进更新。

## 状态图例

| 标记 | 含义 |
|---|---|
| ✅ | 已实现（QEMU 验证通过） |
| ◻ | 规划中（主线里程碑，见 DEVELOPMENT.md） |
| ○ | 值得 / 可选（P2–P3） |
| ✖ | 明确不做（边界） |

---

## 1. 内核基础能力（minimal 模式，≤32MB）

| 特性 | 说明 | 状态 | 里程碑 | 出处 |
|---|---|---|---|---|
| 多协议启动 | 自包含 multiboot1 扁平（QEMU）/ PVH ELF / multiboot2（GRUB）三协议 | ✅ | M0 | DESIGN §1.3 |
| 长模式 + 页表 | 32→64 位切换、1GB 2MB 大页恒等映射 | ✅ | M0 | DESIGN §1.3 |
| 串口输出 | 手写 8250 UART，115200-8N1，`println!`/panic 落串口 | ✅ | M0 | DESIGN §1.4 |
| VGA 文本屏 | 0xB8000 文本模式（80×25），串口输出镜像上屏（QEMU screendump 截屏验证） | ✅ | M1 | — |
| 中断/异常 | 手写 IDT（0–31 异常）+ 8259A PIC 重映射 | ✅ | M0 | DESIGN §1.4 |
| 内存映射打印 | multiboot1/2/PVH 三种启动信息解析 | ✅ | M0 | — |
| GDT + TSS + IST | 独立栈（DF/NMI/MC/DBG） | ◻ | M2 | DESIGN §1.4 |
| 物理内存 | Buddy（order 0–10，分裂/合并/`load_end=0` 修正）+ SLUB 风格 Slab（**侵入式空闲链表**）+ GlobalAlloc（Vec/Box 可用）+ OOM 回调 + **可移动页标记**（自测 ALL PASS） | ✅ | M1 | DESIGN §3.1/勘误§9-10 |
| 虚拟内存 | M2 切片：每任务地址空间（VMA 表）+ **懒分配**（首次访问分配页）+ **COW 写时复制**（fork 共享物理页，写分离，QEMU 实测父 P1/子 P2 独立）；4 级页表 + CR3 切换（M3 用户态接入）+ **内存紧缩 compact_zone** | ✅ 切片 / ◻ 完整 | M2/M9 | DESIGN §3.2 |
| 任务/调度 | M2 完成：内核线程 + 上下文切换 + **CFS（vruntime 红黑树，固定池无 IRQ 分配）** + 权重调度（prio→权重，实测 2:4:8 CPU 占比）+ 睡眠/阻塞唤醒 + **PIP 有效优先级**；**per_cpu! 宏 + `cpu_rq(cpu_id)` 占位（SMP 预热）**；RT 类双队列预留（M9+） | ✅ 切片 / ◻ RT | M2 | DESIGN §4.2/勘误§7/§11 |
| RT 调度类 | **SCHED_FIFO 基本模型**（优先级 + 抢占，Modbus 等硬实时场景 100ms 响应）——从 M2 双队列预留固化 | ◻ | M9 | DESIGN §21.7 |
| 网络调试开关 | `echo 1 > /proc/sys/net/shanshui-guanxin/packet_trace`：环形日志打印每包**五元组 + 丢弃原因**（性能降 ~50%，仅供调试，替代 tcpdump，✅ M13，shell `echo` 已支持 `>` 重定向） | ◻ | M13 | DESIGN §21.8 |
| 同步原语 | Spinlock（关中断自旋）+ 阻塞 Mutex（**内置优先级继承 PIP**，等待者提升持锁者、解锁恢复）；锁序编译期编码 + **RT 强制自旋锁 + CFS 关抢占**（勘误 §11 后续切片） | ✅ 切片 / ◻ 完整 | M2 | DESIGN §3.9/勘误§11 |
| 定时器/时钟 | PIT 8254 tick（100Hz）已接入调度；最小堆 + 时钟源抽象 + RTC + monotonic + **分层时间轮（评估）** | ✅ 切片 / ◻ 完整 | M2/M9 | DESIGN §6.2⑥/勘误§5 |
| 系统调用 + init/shell | syscall 表 + ELF 加载 + 用户态 shell + **PID 1 崩溃自愈（rescue_init + watchdog 复位）** | ◻ | M3 | DESIGN §1.2/勘误§3 |
| VFS + ramfs/tmpfs | Inode/Dentry/SuperBlock + dcache LRU + **最小记录锁** | ◻ | M4 | DESIGN §3.6 |
| 网络栈 | 完整 TCP/IP（重传/拥塞控制）+ UDP/ICMP/ARP + epoll + **SNTP 客户端** + **零拷贝 Skb 内存池（评估）** | ◻ | M5 | DESIGN §3.8/勘误§4 |
| Namespace | pid/mnt/net/uts/ipc/user/cgroup 七类 | ◻ | M6 | DESIGN §3.4 |
| Cgroup v2 | memory/pids/cpu 控制器 + OOM-kill（容器内） | ◻ | M6 | DESIGN §3.5 |
| OverlayFS | lower/upper/work + **稀疏 copy-up** + whiteout + **容器日志默认 tmpfs** | ◻ | M7/M8 | DESIGN §3.7/勘误§1 |
| 容器运行时 | 类 runC：clone → namespace → pivot_root → exec | ◻ | M8 | DESIGN §4.6 |
| 网关 | IP 转发 + conntrack + NAT/MASQUERADE + 基础防火墙 | ◻ | M8 | DESIGN §4.7 |
| 快速启动 | 秒级冷启动：deferred init + 只初始化必需驱动 | ◻ | M9 | DESIGN §19.1 |
| 内存预算（含 Page Cache） | `total_inactive_file` 可回收 + `vm.dirty_ratio=5%` 缩紧（防缓存吞内存） | ◻ | M9 | DESIGN §5.3 |
| 看门狗 / 掉电保护 | 硬件 watchdog + 日志原子写 + FS 一致性 | ◻ | M9 | DESIGN §19.2 |
| 可观测性 | 环形日志 + 落盘 + 健康指标（内存/fd/CPU） | ◻ | M9 | DESIGN §19.2 |
| GDB 调试 | panic 可读化 + crash dump（远程诊断） | ◻ | M9 | DESIGN §19.2 |

## 2. 容器与 OTA（full 模式，≤40MB）

| 特性 | 说明 | 状态 | 里程碑 | 出处 |
|---|---|---|---|---|
| ext4 文件系统 | Block I/O + Page Cache + ext4 驱动（**data=journal 完整模式**，journal buffer 约 5–10% 内存）+ **电梯调度（评估）** | ◻ | M10/M13 | DESIGN §13.3/勘误§6 |
| 动态链接 | ELF PT_INTERP + ld-musl + MAP_SHARED | ◻ | M11 | DESIGN §13.6 |
| futex | WAIT/WAKE/REQUEUE，**逻辑键**（Inode/虚拟区）+ **COW 等待队列迁移** | ◻ | M11/M4 | DESIGN §13.7/勘误§2 |
| TLS | arch_prctl(ARCH_SET_FS) + 上下文切换恢复 | ◻ | M11 | DESIGN §13.8 |
| 设备框架 | devtmpfs + devpts + `/dev/null/zero/urandom` + PTY | ◻ | M12 | DESIGN §13.4 |
| Capabilities | Linux capability 集（permitted/effective/...） | ◻ | M12 | DESIGN §13.5 |
| Seccomp BPF | 最小解释器 + **高风险调用参数值匹配**（mount/ptrace/openat/execve/reboot/clone） | ◻ | M12 | DESIGN §13.5/勘误§12 |
| 完整 /proc | /proc/self/{maps,status,exe,fd} + cpuinfo/mounts/filesystems + **meminfo**（✅ M13-14 内存基线：buddy/slab 台账 + used+free==total 守恒）（**maps ✅ M13-01、status ✅ M13-02、exe ✅ M13-03、fd ✅ M13-04、mounts/filesystems ✅ M13-05**） | ◻ | M13 | DESIGN §13.12 |
| 完整信号 | sigaction(SA_SIGINFO/SA_ONSTACK) + **SIGSEGV 投递**（用户态 #PF → handler/终止，✅ M13-06/10）+ **sigaltstack 备用栈**（✅ M13-07）+ **sigprocmask 阻塞语义 + kill 投递**（✅ M13-08）+ **sigreturn 增强**（handler 期间阻塞本信号 SA_NODEFER 豁免 + rt_sigreturn 恢复 mask，✅ M13-09）+ **JVM 运行时面冒烟**（timerfd+epoll 事件循环 + 信号，✅ M13-13）、实时信号 | ◻ | M13 | DESIGN §13.10 |
| timerfd / signalfd | 事件循环 fd（**timerfd ✅ M13-11**：create/settime/gettime + epoll EPOLLIN + read 计数；**signalfd ✅ M13-12**：signalfd4 + kill 消费 + read siginfo） | ◻ | M13 | DESIGN §13.11 |
| **OCI 镜像** | `shanshui-guanxin-pull`：registry HTTPS + SHA-256 校验 + 层解压 | ◻ | M14 | DESIGN §16 |
| **轻量容器运行时** | 生命周期 + overlayfs 组装（不做 docker daemon/CLI） | ◻ | M14 | DESIGN §16 |
| **OTA 升级 + 回滚** | 层增量拉取 + 镜像版本切换 + 一键回滚；**内核镜像纳入 A/B 分区管理**（内核分区 A/B 标识 + 回滚，覆盖内核本身升级） | ◻ | M14 | DESIGN §16/§21.9 |
| 离线导入 | `docker save` tar → Web 上传 / U 盘拷入 | ◻ | M14 | DESIGN §20.2 |
| veth / bridge | 容器网络 + DNAT 端口映射 | ◻ | M14 | DESIGN §13.11 |

## 3. 工具链与语言支持

| 特性 | 说明 | 状态 | 里程碑 | 出处 |
|---|---|---|---|---|
| 交叉编译工具链 | musl 静态子集：**宿主机**交叉编译（Go/Rust/C++），设备端不编译 | ◻ | M11 | DESIGN §15 |
| Go 交叉编译 | `GOOS=linux CGO_ENABLED=0`，宿主机编译 → OTA 下发 | ◻ | M11 | DESIGN §15.1 |
| Rust 交叉编译 | `x86_64-unknown-linux-musl` 现成 target（宿主机） | ◻ | M11 | DESIGN §15.1 |
| C++ 交叉编译 | musl-cross + `-static -static-libstdc++ -static-libgcc`（宿主机） | ◻ | M11 | DESIGN §15.1 |
| **山水观心 SDK 基础镜像** | ld-musl + 头文件 + linker script，`--dynamic-linker` 指向 `/shanshui-guanxin/` 专用路径 | ◻ | M11 | DESIGN §15.2 |
| **shanshui-guanxin-check 工具** | ELF syscall 依赖扫描 + 内存足迹预估（RSS+虚拟内存）——应用合入门槛；**启动前扫描 PT_INTERP，非 `/shanshui-guanxin/ld-musl` 拒绝启动并提示（glibc 拦截）** | ◻ | M11 | DESIGN §15.3/§21.1 |
| **官方软件仓库** | "小而精"精选集合（core/runtime/service/net-tools），预编译 + 预配置 + 签名 + musl 完全兼容；`shanshui-guanxin install redis` 开箱即用，内置部署模板（DESIGN §22） | ◻ | M11（清单）/ 长期 | DESIGN §22 |
| **`shanshui-guanxin-build` 工具** | 一键从源码构建 山水观心操作系统兼容软件包（阶段一）；源码 → OCI 镜像 + 自动 `shanshui-guanxin-check` 验证（阶段二） | ◻ | M11 | DESIGN §22.3 |
| **`shanshui-guanxin` 软件仓库 CLI** | `shanshui-guanxin repo-add`/`shanshui-guanxin install`（阶段二社区仓库）/`shanshui-guanxin deploy redis`（阶段三云端商店） | ◻ | 长期 | DESIGN §22.3 |
| ABI 契约文档 | `docs/abi.md`：syscall 白/黑/灰名单 + 结构体/errno/调用约定 | ◻ | M11 | DESIGN §15.3 |
| Lua / MicroPython / QuickJS | 轻量脚本运行时 | ○ | M14 | DESIGN §18.2 |
| CPython / JVM | musl 构建可行性评估通过才做 | ○ | P3 | DESIGN §18.2 |

## 4. 应用与服务组件

| 特性 | 说明 | 状态 | 里程碑 | 出处 |
|---|---|---|---|---|
| Redis | 缓存 + 消息（Streams/Pub-Sub），外部 TCP 访问（**部署模板**：`--maxmemory 64mb --maxmemory-policy allkeys-lru`、禁 RDB、只开 AOF+重写；**预置只读 redis.conf，防用户覆盖导致 OOM-kill**） | ◻ | M14 | DESIGN §18.3/§21.2 |
| SQLite | musl 静态库，CRUD + WAL 持久化（依赖最小记录锁） | ◻ | M14 | DESIGN §18.3 |
| Mosquitto (MQTT) | IoT 设备接入 broker，Pub/Sub + QoS | ○ | M14 | DESIGN §18.4 |
| Modbus 工业协议 | 采集侧：RTU/TCP 读寄存器 → JSON | ○ | M14 | DESIGN §18.5 |
| Web 管理界面 | 轻量 HTTP + 静态前端（设备管理标配） | ○ | M14 | DESIGN §20 |
| SSH（dropbear） | 开发层远程调试/救援 | ○ | M14 | DESIGN §20 |
| Agent 主动上联 | 云平台规模化部署 + 远程 OTA | ○ | M14 | DESIGN §20 |
| JSON / CSV | 语言库自带，设备数据交换基础 | ◻ | — | DESIGN §18.5 |
| HTTP 客户端 | SSE 流式响应 + 长超时 + 大 JSON 流式解析（AI=HTTP 用例） | ○ | M14 | DESIGN §18.6 |
| 轻量 TLS | mbedTLS / rustls（HTTPS 通道） | ○ | M14 | DESIGN §18.5 |

## 5. 安全能力

| 特性 | 说明 | 状态 | 里程碑 | 出处 |
|---|---|---|---|---|
| 最小攻击面 | 最小 syscall 集 + 静态驱动（无模块加载） | ✅ 设计 | — | DESIGN §9 |
| 容器隔离 | namespace + cgroup + user ns（容器内 root ≠ 宿主 root） | ◻ | M6/M8 | DESIGN §9.2 |
| **应用合入门槛** | 外部应用移植必须先通过 `shanshui-guanxin-check`（syscall 依赖 + 内存足迹预估），否则禁止合入 | ◻ | M11/M14 | DESIGN §15.3 |
| unsafe <5% | 显式标注 + SAFETY 审查边界 | ✅ 约束 | 全程 | DESIGN §6.3⑥ |
| 内存预算强制 | 子系统 used/limit 台账 + CI 断言 ≤32MB/40MB | ◻ | M9 | DESIGN §5.3 |
| Secure Boot | 内核签名 + 信任根（产品级准入项） | ○ | 产品级 | DESIGN §19.3 |
| 合规 | IEC 62443 / ISO 26262（目标行业需要时） | ○ | 产品级 | DESIGN §19.3 |

## 6. 交互模式（无头设备，见 interaction.md）

| 层级 | 通道 | 状态 | 里程碑 |
|---|---|---|---|
| 用户层 | Web 管理界面（`http://<设备IP>`，拉取/运行/看状态） | ○ | M14 |
| 开发层 | SSH（dropbear）/ 串口 Console | ○ | M14 |
| 运维层 | Agent 主动上联（云平台规模化部署 + OTA） | ○ | M14 |

## 7. 架构级预留（第一版定型，防返工）

| 维度 | 内容 | 状态 |
|---|---|---|
| 设备驱动模型 | bus→device→driver + BSP + 中断分发（GPIO/I2C/SPI/CAN/UART/PWM/ADC） | ✅ 设计定型 |
| RT 调度双队列 | RT 类（优先级+抢占）+ 普通类（CFS）+ **优先级继承 PIP** | ✅ 结构预留 |
| 时钟/中断框架 | 时钟源抽象 + RTC + monotonic | ✅ 设计定型 |
| 快速启动 | deferred init 启动路径 | ✅ 设计定型 |
| ARM64 / RISC-V | arch 层隔离 + 双留口；**PlatformInfo 抽象**（x86=Multiboot/ARM=DTB 统一）、**ACPI/设备树解析**、**SMP per-CPU 预热 + 跨核负载均衡**、**SNTP 时间同步** | ✅ 设计定型 + 中期补 |

## 8. 明确不支持（边界）

| 能力 | 原因 |
|---|---|
| glibc 生态（mysql / rabbitmq / kafka / PostgreSQL） | 掉进"glibc 兼容层"陷阱，价值递减（DESIGN §14） |
| 任意 Docker 镜像兼容（daemon/CLI） | 只跑"为 山水观心操作系统编译"的 musl 镜像（DESIGN §16） |
| 桌面 / 手机 / 平板 | 无 GPU/触摸/应用生态 |
| 高负载服务器 | Linux 更合适，内存优势无用武之地 |
| MCU 级（ESP32/STM32） | 内存 KB 级，跑不了 MMU 内核 |
| 安全关键设备（医疗核心/航空） | 需认证合规 |
| 智能机器人主控 | ROS2/GPU/NPU/RT 生态 |
| Excel / docx / PHP | 设备不处理文档格式；PHP 非"前端美观"答案 |

## 9. 用户需求演进（roadmap 定心丸，只增不减）

> 基于三大用户画像的高频新需求，集中在**硬件连接性 / 服务可靠性 / 远程可运维性**。
> 详见 DESIGN §23；原则：内存优先、默认禁用、模块化（feature flag 裁剪）。

| 特性 | 说明 | 状态 | 版本 | 出处 |
|---|---|---|---|---|
| 容器保活策略 | OCI `restartPolicy`：`always`/`on-failure`/`unless-stopped`（掉电自启无人值守） | ◻ | v1.1 | DESIGN §23.1 |
| Web 管理界面 | 默认开启（端口 80）：容器列表/启停/日志滚动/资源曲线 | ◻ | v1.1 | DESIGN §23.1 |
| 4G/5G 蜂窝上网 | PPP 协议栈 + USB 串口驱动 + wpa_supplicant 轻量移植（Wi-Fi WPA2/WPA3） | ◻ | v1.5 | DESIGN §23.1 |
| 持久化日志 | 内核/容器日志异步落盘 `/var/log/journal/` + 按大小轮转 + 总大小限制 | ◻ | v1.5 | DESIGN §23.1 |
| WireGuard VPN | `shanshui-guanxin-vpn` 站点到站点安全连接（代码量小，32MB 友好） | ◻ | v1.5 | DESIGN §23.1 |
| 存储卷独占锁 | `shanshui-guanxin run --volume-exclusive`（flock/leases），防 SQLite 并发写损坏 | ◻ | v1.5 | DESIGN §23.1 |
| MicroPython 运行时 | <256KB 脚本引擎（工业数据清洗），官方镜像入仓库 | ◻ | v1.5 | DESIGN §23.1 |
| PTP/NTP 时间同步 | SNTP 升级 chrony/ntpd 轻量版（时钟漂移补偿）+ 评估 PTP(IEEE 1588) | ◻ | v1.5 | DESIGN §23.1 |
| 四层负载均衡（L4LB） | 基于 IPVS 的轮询分发（多容器同服务） | ◻ | v2.0+ | DESIGN §23.2 |
| 流量镜像 | 复制流量送测试容器做灰度验证 | ◻ | v2.0+ | DESIGN §23.2 |

---

*状态随实现推进更新；对应路线图见 DEVELOPMENT.md，设计依据见 DESIGN.md，参考组件见 REFERENCES.md。*
