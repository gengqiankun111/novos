# 山水观心操作系统（Shanshui-guanxin）

> **山水观心操作系统**（英文名 **Shanshui-guanxin**，CLI 代号 `shanshui-guanxin`）：在内存受限的边缘设备上，跑起真实容器服务的最小化操作系统。

山水观心操作系统是一个用 Rust 从零编写的嵌入式操作系统，瞄准 FreeRTOS 与嵌入式 Linux 之间一块真实存在、但一直没人占住的缝隙：
在 **256MB–2GB 内存**的边缘设备（工业网关 / 工控盒子 / IoT 节点 / SD-WAN CPE）上，用 **≤32MB 内核常驻内存**跑起多服务隔离的容器环境。

它只保留容器基础设施必须的最小闭环——完整 TCP/IP 网络栈、epoll、Namespace、Cgroup、OverlayFS——砍掉了通用 Linux 数十年的兼容包袱和用不到的驱动，并兼容 Linux syscall ABI + musl 静态子集。

> ⚠️ **当前状态**：早期开发阶段，仅有自举/启动引导完成，容器、网络、隔离等能力正按里程碑推进（见[路线图](#路线图)）。本文描述的是目标能力与规划，实际进度以里程碑为准。

---

## 为什么需要它

边缘计算的容器化正在下沉到设备，但现有方案两头够不着：

| 方案 | 问题 |
|---|---|
| RTOS（FreeRTOS / Zephyr） | 无 MMU、单进程，做不到"多服务隔离 + 容器" |
| 裁剪版 Linux（Yocto / OpenWrt） | 空闲就占 50–80MB，256MB 设备跑 2–3 个容器就超 200MB；启动慢、攻击面大、抖动高 |

山水观心操作系统专为这中间的窄缝隙而生：**多服务隔离 + 可更新 + 低延迟，确定性优先**。相比裁剪 Linux，它在编译期就去掉了"不需要的代码"，从根源上解决了内存地板太高、可审计性差、安全攻击面大的问题。

---

## 适合谁用

| 人群 | 你用它做什么 |
|---|---|
| **嵌入式 / 边缘设备工程师**（主目标） | 给网关、工控盒、IoT 设备选型 OS、移植到板子、把服务容器化部署上去 |
| **云原生 / 应用开发者** | 用 Go / Rust / C++ 写容器化服务，宿主机交叉编译，`shanshui-guanxin-pull` 部署到设备 |
| **设备运维 / 方案集成商** | 规模化部署、远程 OTA、Web 界面 / SSH / 云端平台统一管理 |

**不适合**（Core 标准版）：通用 Linux 发行版替代、桌面/图形工作站、高吞吐存储服务器、需要 glibc 生态（MySQL / Kafka 等）、MCU 级设备（ESP32/STM32）、GPU 强依赖设备 —— 桌面/图形需求请选 **Desktop 图形版**（见下方"产品线"）。

---

## 核心能力

**你能用它做什么：**
- **在 256MB–2GB 设备上运行容器化服务**：多服务隔离（Namespace + Cgroup）、可更新、低延迟
- **当网络网关用**：完整 TCP/IP + NAT + conntrack + 基础防火墙 + **shanshui-guanxin-gateway**（HTTP/1.1 + TLS + 反向代理）
- **秒级冷启动 + 确定性调度**：低抖动、可预测的延迟
- **OTA 升级与回滚**（full 模式）：层增量拉取、镜像版本切换
- **可观测性**：`top` 系统监控（Rust 静态编译 <100KB）+ 健康指标 + Web 版 `/top`（远期）
- **三种远程运维方式**：Web 管理界面（浏览器访问设备 IP）、SSH / 串口 Console、云端 Agent 主动上联
- **图形桌面**（Desktop 版）：显示器/触摸屏本地操作、GUI 系统监视器、类 Windows 文件视图（文档/下载/桌面）

**产品线：**

| 产品线 | 编译档 | 内核常驻 | 定位 | 适用 |
|---|---|---|---|---|
| **Core**（标准容器宿主） | minimal | ≤32MB | 微型容器宿主（静态 musl 子集） | 边缘网关、工控、IoT（256MB–2GB 设备） |
| | full | ≤40MB | 容器服务宿主（ext4 + 动态链接） | 磁盘持久化 + Redis/SQLite + 交叉编译 |
| **Desktop**（图形桌面版） | full + gui | ≥128MB | 容器宿主 + 帧缓冲/DRM + Wayland + 桌面应用 | 需要本地可视化、监控面板直显、图形化配置 |

**内置生态（规划/推进中）：**
- 🟢 必支持：Redis、SQLite、**shanshui-guanxin-gateway**（HTTP/反向代理）、**top**（系统监控）、busybox、musl 交叉工具链、JSON/CSV、MQTT 客户端
- 🟢 值得：Mosquitto（MQTT broker）、Modbus 工业协议、内置 Web 管理界面、轻量 TLS、Lua / MicroPython / QuickJS
- **语言**：C / Rust / Go（musl 静态）优先；Lua / MicroPython / QuickJS 可选
- **典型价值闭环**：Modbus 采集 → JSON → MQTT/HTTP 上报 → Web 界面监控

---

## 快速开始（开发 / 演示）

需要 nightly Rust + QEMU + musl 工具链。

```bash
# 1. 准备环境
rustup toolchain install nightly
rustup target add x86_64-unknown-none
# Ubuntu: sudo apt install qemu-system-x86 musl-tools

# 2. 构建内核镜像
make build          # minimal 模式（32MB）
make build-full     # full 模式（~40MB，Linux 兼容）

# 3. 在 QEMU 里跑起来
make run
```

> Windows 上可以在 QEMU 里跑山水观心操作系统做开发/演示，生产负载放 Linux/KVM。开发期与生产期的交互完全一致。

**部署一个容器（以 Redis 为例）：**
- **手动**：Web 界面点"拉取镜像"（或设备端 `shanshui-guanxin-pull` 连 registry，HTTPS + token + SHA-256 校验）→ 解压到本地镜像仓库 → 点"运行"查看状态；
- **自动**：设备上电 → Agent 上联云平台 → 下发"部署 xx 版本" → 自动 pull/校验/运行 → 回报状态/日志 → 异常一键回滚；
- **离线**：可上网的电脑 `docker save` 导出 tar，Web 上传或 U 盘拷入。

---

## 在真实设备上使用

**最匹配的设备**（直接对上强项）：

| 设备场景 | 典型硬件 | 做什么 |
|---|---|---|
| 工业协议网关 | ARM 工业盒子（RK3568、IMX6/8、全志）、x86 工控机 | Modbus/PLC 采集 → JSON → 上云 |
| IoT 汇聚网关 | 树莓派 3/4、RK 系盒子 | Zigbee / 蓝牙 / 串口设备汇聚 → MQTT 上云 |
| SD-WAN / 软路由 / VPN 网关 | x86 软路由小主机、ARM 路由板 | 自研网络栈 + NAT + 容器化服务 |
| 能源数据采集 | ARM 工控板 | 光伏逆变器 / 电表 / 充电桩数据采集上报 |
| 环境监测站 | 低配 ARM 板 | 传感器采集 + 定时上报（气象 / 水质） |

**也可匹配**：零售/商业终端（智能收银、自助终端、广告屏主控）、车载非实时设备（T-Box 数据记录/远程诊断/OTA）、云边缘轻量节点（低配 ECS/边缘实例）、旧设备再利用（512MB–2GB 旧 x86 小主机）。

> 注意：内核驱动以 UART / 定时器 / virtio 起步，USB Host 最小集与更多外设驱动会跟着锁定的目标设备逐步补齐，不预先全做。

---

## 从 Linux / Docker 迁移（三个必踩的坑）

这三个点最反直觉，决定第一印象：

1. **只支持 musl，不支持 glibc** —— 用 `apt-get install` 或 Ubuntu 工具链编译的程序会直接段错误。必须用宿主机交叉编译（Go/Rust/C++ 现成 target）。容器启动前 `shanshui-guanxin-check` 会扫描，非 musl 一律拒绝启动并提示。
2. **Ext4 只认 `data=journal`** —— 现成 Linux 默认 `data=ordered` 的盘 mount 会报 `Operation not supported`。用 `tune2fs -O journal_dev` 转换或重新格式化。
3. **设备端不能编译** —— 设备上没有 go/rustc/g++（内存装不下）。别在设备上 `go build`（会卡死/OOM）。

**其他预期管理：**
- 容器管理是 `shanshui-guanxin` 命令，**不是 Docker CLI**（不支持 docker-compose）；
- Redis 预置只读配置（防 OOM-kill 数据丢失）；
- `/proc/cpuinfo` 报告硬件核心数但 online 只有 1（v1.0 单核，SMP 见[路线图](#路线图)）；
- 网络排查：`echo 1 > /proc/sys/net/shanshui-guanxin/packet_trace`（无需 tcpdump）。

---

## 常见问题

**Q：32MB 够用吗？容器也要内存啊。**
32MB 是**内核常驻开销**，容器应用的内存通过 Cgroup 逐容器独立核算，不计入内核预算。32MB 只管"内核自己吃多少"。

**Q：能跑标准 Docker 镜像吗？**
第一版目标是跑 musl 静态链接的 ELF（如 busybox 静态版）。标准 Docker 镜像依赖 glibc 动态链接，长期目标是兼容 OCI image spec。MySQL/Kafka 等 glibc 生态镜像不追。

**Q：支持多核（SMP）吗？**
v1.0 单核（UP），SMP 是 v2.0 核心目标（per-CPU 运行队列 / IPI / 负载均衡已在架构层预留，打开后内存增量约 1–2MB）。

**Q：能 `apt install` 吗？**
不能。apt/dpkg 是 glibc 生态，与 musl 静态子集定位冲突；full 模式用官方交叉编译工具链分发软件。JVM / Python 均为远期可选，需先评估 musl 构建。

**Q：为什么不用 seccomp + AppArmor 裁剪 Linux，而要自己写内核？**
seccomp/AppArmor 裁剪的是运行时行为，不改变编译期代码量和内核常驻内存。从零写内核才能把"不需要的代码"在编译期彻底删除。

**Q：和 Redox OS 有什么区别？**
Redox 是通用微内核 OS（约 200MB+，有窗口系统、包管理器）；山水观心操作系统是专用内核，Core 只做"容器宿主 + 网关"（目标 32MB），Desktop 版另提供图形桌面（≥128MB），定位完全不同。

---

## 与同类项目对比（选型参考）

| 维度 | 裁剪 Linux | OSv / MirageOS | **山水观心操作系统** |
|---|---|---|---|
| 定位 | 通用容器宿主 | 单应用 unikernel | **网关 + 容器宿主内核** |
| 常驻内存 | 50–80MB | 10–30MB | **≤32MB** |
| 多容器支持 | 是 | 否（单应用） | **是（Namespace + Cgroup）** |
| 网络栈 | 完整 Linux | 简化 | **完整 TCP/IP + epoll** |
| 网关能力 | netfilter/iptables | 无 | **NAT + conntrack + 防火墙** |
| Linux 兼容 | 完全 | 部分 | **musl 静态链接兼容** |

OSv / MirageOS 更小但牺牲了多容器隔离；Linux 通用但太重。山水观心操作系统在"能跑多容器"和"常驻够小"之间找到了位置。

---

## 路线图

| 里程碑 | 内容 |
|---|---|
| M0–M8 | 从"能启动"到"跑起第一个容器"（内核 → 网络栈 → 隔离 → OverlayFS → 容器运行时 + 网关） |
| M9 | 内存基线 ≤32MB + 长期稳定版（生产可用）+ top 系统监控 |
| M10–M14 | ext4 磁盘持久化、动态链接、Docker 安全模型、OCI 镜像 + OTA（Redis/SQLite）、shanshui-guanxin-gateway、full 模式 ≤40MB |
| M15+ | ARM64 / RISC-V 评估、电源/Flash/调试等功能级补强 |
| M16（Desktop） | 帧缓冲 `/dev/fb0` → DRM KMS → Wayland 合成器 → 桌面应用（≥128MB，独立产品线） |
| v1.1 / v1.5 | 容器保活、shanshui-guanxin-gateway、Web 界面默认开启、4G/5G 蜂窝、WireGuard、PTP/NTP 时间同步 |
| v2.0 | SMP 多核、L4 负载均衡、流量镜像、帧缓冲 + DRM（Desktop 阶段一） |
| v3.0+ | Wayland 合成器 + 系统监视器（Desktop 阶段二）；v4.0 完整桌面环境 |

> **M13 进行中**：完整 /proc 视图（`/proc/self/{maps,status,exe,fd}` + `/proc/{mounts,filesystems,cpuinfo,health}`）✅；
> 信号子系统（`rt_sigaction` SA_SIGINFO/SA_ONSTACK、用户态 #PF→SIGSEGV 投递、`sigaltstack` 备用栈、`sigprocmask` 阻塞 + `kill`）✅；
> timerfd（`timerfd_create/settime/gettime` + epoll 阻塞监听 + `read` 到期计数）✅；
> signalfd（`signalfd4` + kill 信号消费 + epoll 就绪 + `read` siginfo）✅；
> 网络调试开关（`echo 1 > /proc/sys/net/shanshui-guanxin/packet_trace` 环形日志五元组 + 丢弃原因，shell `>` 重定向）✅；
> 均经 QEMU `sigtest`/`sigmasktest`/`tfdtest`/`sftest`/`pktracetest` 集成验证。剩余：实时信号、JVM 冒烟、内存基线（详见 [DEVELOPMENT.md](DEVELOPMENT.md)）。

---

## 文档索引

| 文档 | 内容 |
|---|---|
| [docs/DESIGN.md](docs/DESIGN.md) | 设计文档（架构、内存预算、核心算法） |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | 开发路线图与里程碑、贡献指南 |
| [interaction.md](interaction.md) | 无头设备三层远程通道（Web / SSH / Agent） |
| [FEATURES.md](FEATURES.md) | 功能说明书与状态 |
| [DESIGN_ERRATA.md](DESIGN_ERRATA.md) | 设计勘误与补救 |
| [REPOSITORY.md](REPOSITORY.md) | 软件仓库规范（NRS） |

---

## License

Mulan-PSL-2.0（木兰宽松）。
