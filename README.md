# Novos‑OS

> **面向内存受限设备的微型容器宿主 —— RTOS 的占地，Linux 的生态，Rust 的安全。**
>
> 用 Rust 从零编写，定位在 FreeRTOS 与嵌入式 Linux 之间那块真实存在、但没人占住的缝隙：
> 在 **256MB–2GB 内存的边缘设备**（网关 / 工控盒子 / IoT / SD-WAN CPE）上，用 **≤32MB 内核常驻** 跑起真实的容器服务。

Novos‑OS 不是 Linux 的复制品。它砍掉了数十年来的兼容包袱和成千上万个用不到的驱动，只保留容器基础设施必须的最小闭环：**TCP/IP 完整网络栈 + epoll + Namespace + Cgroup + OverlayFS + 调度/内存管理**，并兼容 Linux syscall ABI + musl 静态子集（自己编译，不追现成二进制）。

---

## 定位

> 一句话：**面向内存受限设备的微型容器宿主** —— 多服务隔离 + 可更新 + 低延迟，确定性优先。

### 定位决策

| 项 | 决策 |
|---|---|
| 主定位 | 嵌入式容器宿主（确定性 + 安全容器网关） |
| 目标设备 | 边缘网关、工控盒子、IoT 设备、SD-WAN CPE（内存 256MB–2GB） |
| 架构 | x86_64 起步（QEMU 开发便利），**ARM64 为终局目标**（arch 层隔离从第一天做好） |
| 明确不做 | 通用 Linux 替代、桌面、大众市场、完整 glibc 生态（mysql/kafka 等） |
| 兼容策略 | Linux syscall ABI 兼容 + musl 静态子集（Go/Rust/C++ **宿主机交叉编译**，见 DESIGN §15） |
| 交付形态 | 官方交叉编译工具链 target + 垂直场景方案，而非"内核"单品 |

### 目标设备分级

**✅ 最匹配（首推，直接对得上强项）**

| 设备场景 | 典型硬件 | 做什么 |
|---|---|---|
| **工业协议网关** | ARM 工业盒子（RK3568、IMX6/8、全志）、x86 工控机 | Modbus/PLC 采集 → JSON → 上云 |
| **IoT 汇聚网关** | 树莓派 3/4、RK 系盒子 | Zigbee / 蓝牙 / 串口设备汇聚 → MQTT 上云 |
| **SD-WAN / 软路由 / VPN 网关** | x86 软路由小主机、ARM 路由板 | 自研网络栈 + NAT + 容器化服务 |
| **能源数据采集** | ARM 工控板 | 光伏逆变器 / 电表 / 充电桩数据采集上报 |
| **环境监测站** | 低配 ARM 板 | 传感器采集 + 定时上报（气象 / 水质） |

**🟡 可匹配（次选，能用但非核心场景）**：零售/商业终端（智能收银、自助终端、广告屏主控）、车载非实时设备（T-Box 数据记录/远程诊断/OTA，**排除电机控制/底盘实时**）、云边缘轻量节点（低配 ECS/边缘实例）、旧设备再利用（512MB–2GB 旧 x86 小主机）。

**❌ 明确不适用（边界划死）**

| 排除场景 | 原因 |
|---|---|
| 智能机器人主控 | ROS2/GPU/NPU/RT 生态，前面已论证 |
| 手机 / 平板 / 桌面 PC | 触摸 / GPU / 应用生态，需要桌面级 |
| 高负载服务器 | 大数据 / 高并发，Linux 更合适，内存优势无用武之地 |
| MCU 级（ESP32/STM32） | 内存 KB 级，跑不了 MMU 内核——那是 RTOS / 裸机地盘 |
| GPU 强依赖设备（视觉 AI 盒子） | 没有 GPU/NPU 驱动，除非只跑 CPU 小模型 |
| 安全关键设备（医疗核心 / 航空） | 需要认证合规，不是自研内核能碰的 |

### 为什么是这个定位（结构性空白）

- **需求侧**：边缘计算 + 容器化下沉到设备是真实趋势；这些设备需要"多服务隔离 + 可更新 + 低延迟"。
- **结构性空白**（两边都够不着）：
  - RTOS（FreeRTOS/Zephyr）：无 MMU、单进程 → 够不着"多服务隔离 + 容器"；
  - 裁剪 Linux（Yocto/OpenWrt）：空闲 50–80MB、跑 2–3 个容器就 200MB+ → 256MB 设备跑不动，且启动慢、攻击面大、抖动高。
- **竞品挤压**：高端被精简 Linux（Alpine+Docker）蚕食，低端被 Zephyr 抬升 → 中间是**窄而真实**的缝隙。

### 三条现实约束（必须接受）

1. **维护成本**：嵌入式产品生命周期 10–15 年，自研内核意味着长期跟进 CVE / 安全漏洞 / 架构演进；
2. **生态摩擦**：客户的第一句话永远是"能不能跑我现有的程序"——musl 静态子集 vs 完整 Linux 生态，获客阻力真实；
3. **Linux 持续瘦身**：省下的内存，Linux 的裁剪方案也在追，缝隙不会消失但也不会变大。

### 空间评估与决策结论

- 大众市场**不够大**，但**垂直缝隙 + 技术资产够大**（工业/能源/车载/运营商 CPE）；
- 走嵌入式主定位 + 开源教学辅定位（叠加推荐）：**技术资产 + 开源声望 + 可能的垂直商业**；
- **≤32MB 是入场券，不是卖点**；卖点是确定性 + 安全 + 可控（秒级冷启动、低抖动调度、OTA 升级、Rust 内存安全证明）。

### 架构骨架与演进分级（DESIGN §19）

**架构级（第一版必须预留，否则返工成本极高）**：

| 维度 | 预留内容 |
|---|---|
| 设备驱动模型 | bus→device→driver 统一框架 + BSP + 中断分发（GPIO/I2C/SPI/CAN/多路 UART/PWM/ADC） |
| 实时性 / 确定性 | RT 调度类（优先级 + 抢占）+ 普通类（CFS）双队列 |
| 时钟 / 中断框架 | 时钟源抽象 + 高精度 timer + RTC + monotonic |
| 快速启动 | 秒级冷启动：deferred init + 只初始化必需驱动 |
| RISC-V 预留 | `arch/` 目录同时留 aarch64 / riscv64 口 |

**功能级（中期补）**：电源管理（idle/WFI-WFE、门控、suspend/resume）、Flash 文件系统（littlefs/ubifs）、看门狗、掉电保护、可观测性（环形日志/远程/健康指标）、GDB 调试（panic 可读化/crash dump）。
**产品级（商业化才做）**：安全启动（Secure Boot）、完整 OTA（A/B 分区 + 回滚）、IEC 62443 / ISO 26262 合规、轻量远程运维协议。

> **驱动分期**：首期只有 UART/定时器/virtio；**USB Host 最小集**（串口/U 盘/网卡）中期加，Type-C 只当供电通道，音频默认不做。**驱动跟着锁定的目标设备走，不预先全做**——下一步先定第一块真实目标板（ARM 工业板），按外设清单定驱动清单。

> **2026-08 架构评审**：12 项工程问题（OverlayFS 写放大、Futex COW、PID1 自愈、SMP 预热、零拷贝 skb、时间轮、内存碎片化、Seccomp 参数过滤等）及补救方案已入 [DESIGN_ERRATA.md](DESIGN_ERRATA.md)，并同步进 DESIGN/DEVELOPMENT/FEATURES。**三个必改红线**：OverlayFS 稀疏 copy-up、Futex 逻辑键、PID 1 热备 init。

---

## 交互模式（无头远程运维）

> 设备无头（无屏幕键盘），一切交互在电脑端远程操作，核心三层通道由浅入深（详见 `interaction.md`）。

| 层级 | 通道 | 电脑端工具 | 场景 | 是否连线 |
|---|---|---|---|---|
| 用户层 | **Web 管理界面** | 浏览器访问 `http://<设备IP>` | 日常管理、拉取/运行容器、看状态 | 网络远程 |
| 开发层 | **SSH / 串口 Console** | 终端 + dropbear / PuTTY + USB 转串口 | 开发调试、救援 | SSH 远程；串口需物理连线 |
| 运维层 | **Agent 主动上联** | 云管理平台网页 | 规模化部署、远程 OTA | 设备主动连平台 |

**镜像拉取（"docker pull" → `novos-pull`）**：
- **手动**：Web 界面点"拉取镜像" → 设备端 `novos-pull` 连 registry（HTTPS + token）→ SHA-256 校验 → 解压 → 本地镜像仓库 → 点"运行" → 界面显示状态；
- **自动**：设备上电 → Agent 上联平台 → 下发"部署 xx 版本" → 自动 pull/校验/运行 → 回报状态/日志 → 异常一键回滚（OCI 层复用）；
- **离线**：设备连不上公网时，可上网电脑 `docker save` 导出 tar → Web 上传或 U 盘拷入。

> **交互心智统一**：开发期在 Windows 上操作 QEMU 里的 Novos，生产期在浏览器/平台操作真实设备，完全一致。

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
| **minimal** | ≤ 32MB | 微型容器宿主（静态 musl 子集） | 边缘网关、工控盒子、IoT（256MB–2GB 设备） |
| **full** | ≤ 40MB | 容器服务宿主（ext4 + 动态链接） | 磁盘持久化 + Redis/SQLite + 交叉编译 Go/Rust/C++ |

### minimal 模式（32MB，入场券）

| 场景 | 描述 | 为什么选 Novos‑OS |
|---|---|---|
| **边缘容器网关** | 工业网关/IoT 边缘节点上运行容器化的数据采集 + 网络转发 | 32MB 内核常驻 → 256MB 设备也能留内存给业务容器 |
| **确定性容器宿主** | 需要低抖动、秒级冷启动、OTA 可更新 | RTOS 够不着多服务隔离，裁剪 Linux 跑不动 |
| **安全隔离网关** | 极小 TCB 的网络转发 + 容器隔离 | 从零内核 + Rust → 代码量小、可审计、攻击面窄 |

### full 模式（~40MB）

| 场景 | 描述 | 扩展能力 |
|---|---|---|
| **容器服务宿主** | 跑 **Redis + SQLite** 等真实 musl 容器服务 + **OTA 升级回滚**，外部可访问 | OCI 镜像 + 轻量运行时（DESIGN §16，不做 docker daemon/CLI） |
| **开发/演示环境** | Windows 运行器 = 开发/演示环境（生产负载放 Linux/KVM） | futex + TLS + 完整信号 + getrandom |
| **交叉编译工具链** | Go/Rust/C++ 宿主机交叉编译 target 官方支持（设备端不编译） | musl syscall 足迹兼容面（DESIGN §15） |

### 不适合的场景

- **通用 Linux 发行版替代**——不提供 glibc 兼容层，mysql/kafka 等 glibc 生态不追；
- **桌面/图形工作站**——无 GUI、无声卡、无 GPU 栈；
- **高吞吐存储服务器**——无完整 block layer、无 RAID、无文件系统生态。

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
| 开发语言 | **Rust stable**（`no_std` 裸金属 target） | 内存安全 + 零成本抽象 + 稳定通道可用 |
| 目标三元组 | `x86_64-unknown-none`（后续 `aarch64-unknown-none`） | 不依赖宿主 OS，裸金属 |
| 引导方式 | **自包含 multiboot2**（长模式汇编 + 手写页表/IDT） | 无宿主链接依赖，QEMU `-kernel` 直接启动 |
| 用户态 libc | **musl（静态链接，宿主机交叉编译）** | 体积小、兼容 Linux ABI；Go/Rust/C++ 现成 target 复用 |
| 构建系统 | **Cargo + Makefile** | Cargo 管依赖，Makefile 封装 QEMU/测试命令 |
| 测试框架 | `cargo test`（host 逻辑） + QEMU 集成 | 逻辑与硬件解耦，CI 可跑 |
| CI | GitHub Actions | 编译 + `cargo test` + QEMU 启动断言 |
| 模拟器 | **QEMU**（Windows 为开发/演示环境） | 免费、支持 virtio、GDB 调试 |

### 关键依赖 crate（M0 实际）

| crate | 用途 | 是否 `no_std` |
|---|---|---|
| `spin` | 自旋锁原语（串口锁） | 是 |

> 内核自包含：UART/PIC/长模式启动均为手写（对应 REFERENCES.md 的"不引入外部依赖"原则）；后续按需引入无 build script、纯 Rust 的 `no_std` crate。

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
- [ ] veth/bridge（容器网络）
- [ ] **OCI 镜像 + `novos-pull`**（registry HTTPS + 摘要校验 + 层解压）
- [ ] **OTA 升级 + 回滚**（层增量拉取 + 镜像版本切换）
- [ ] 最小记录锁（fcntl 字节区间锁，SQLite 依赖）
- [ ] SQLite 容器服务（musl 静态，CRUD + WAL 持久化）
- [ ] Redis 容器服务（musl 编译，外部 TCP 访问）
- [ ] 交叉编译工具链：宿主机 Go（CGO_ENABLED=0）/ Rust（x86_64-unknown-linux-musl）/ C++（musl-cross），产物 OTA 下发（DESIGN §15）
- [ ] （值得）Mosquitto MQTT broker（musl 静态，IoT 设备接入）
- [ ] （值得）内置 Web 管理界面（轻量 HTTP + 静态前端，设备管理标配）
- [ ] （值得）SSH（dropbear 轻量实现）——开发层远程调试/救援
- [ ] （值得）Agent 主动上联 + 离线导入（`docker save` tar → Web 上传/U 盘）
- [ ] （值得）Lua / MicroPython / QuickJS（轻量脚本运行时）
- [ ] （P3 可选）`apt install` 支持（动态链接 + FHS + HTTPS）
- [ ] （P3 可选）JVM / Python 运行时（需评估 musl 构建）

---

## 支持的生态（软件矩阵）

### 组件矩阵

| 等级 | 组件 |
|---|---|
| 🟢 必支持 | Redis（缓存 + 消息）、SQLite（数据）、轻量 HTTP 服务（网关）、busybox、musl 交叉工具链、**JSON/CSV**（语言库自带，零难度）、**MQTT 客户端** |
| 🟢 值得 | Mosquitto（MQTT broker）、**Modbus 工业协议**（网关核心）、**内置 Web 管理界面**（轻量 HTTP + 静态前端，设备管理标配）、**轻量 TLS**（mbedTLS/rustls）、Lua / MicroPython / QuickJS |
| 🟡 可选 / 远期 | NanoMQ、ZeroMQ、CPython、WebSocket、OPC-UA、边缘本地推理（SNTP 属 M5 主线） |
| ❌ 排除 | ActiveMQ、RabbitMQ、Kafka、MySQL、PostgreSQL、Node、Erlang、**Excel/docx、PHP**（设备不处理文档格式；PHP 不是"前端美观"的答案） |

> **边缘网关价值闭环**（组件只围绕这条链选）：**Modbus 采集 → JSON → MQTT/HTTP 上报 → Web 界面监控**。文档格式不在其中。

### AI 调用评估（并入 HTTP 用例，不单独支持）

- **云端大模型 API = HTTP + TLS + JSON 一个用例**，不需要专门支持；
- HTTP 客户端只需补三条：**SSE 流式响应、长超时、大 JSON 流式解析**（避免一次缓冲）；
- **本地推理**是另一码事（推理引擎 + 数学库 + NPU 驱动），维持**远期独立子系统**结论。

### 语言矩阵（按定位收敛）

| 等级 | 语言 | 理由 |
|---|---|---|
| 🟢 必支持 | **C、Rust、Go**（musl 静态） | 覆盖嵌入式主力，现成 target 复用（DESIGN §15） |
| 🟢 值得 | **Lua**（极轻脚本）、**MicroPython**、**QuickJS**（轻量 JS） | 每个"轻"版本都可行 |
| 🟡 可选 / 远期 | CPython（解释器+stdlib 偏重）、JVM（重、依赖刁钻） | 需要时再评估 |
| ❌ 不推荐 | Node、Erlang/Elixir、.NET | 与轻量定位相反 |

### 消息队列路线（每类需求用最轻的那个）

| 需求 | 方案 | 成本 |
|---|---|---|
| 设备内消息 / 队列 / 发布订阅 | **Redis Streams / Pub-Sub**（Redis 自带，零新增组件） | 🟢 已有 |
| IoT 设备接入（MQTT 协议） | **Mosquitto**（C 写的轻量 MQTT broker，musl 静态顺畅） | 🟢 值得加 |
| 可选：进程间轻量消息 | ZeroMQ（libzmq，C 库） | 🟡 可选 |
| ActiveMQ / RabbitMQ / Kafka | ❌ 排除（Java/Erlang 生态，重且无必要） | — |

### 多线程 vs 协程（不是二选一）

- **多线程是硬需求**：Go goroutine / Rust `std::thread` / C++ 线程 / Redis 多线程全部建立在 OS 线程之上——兼容层必须把 **`clone + futex + TLS`** 做对（地基清单核心项）；
- **协程不需要内核专门支持**：Go/Rust async/Python asyncio 的协程全在用户态实现，内核只需 **epoll + 非阻塞 IO + 时钟**；
- **一句话**：把**线程 + epoll** 做对，多线程和协程同时获得，内核不为协程多付任何工作。

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
| [DESIGN_ERRATA.md](DESIGN_ERRATA.md) | 设计勘误与补救（2026-08 评审：12 项架构问题） |
| [FEATURES.md](FEATURES.md) | 功能说明书：支持什么功能/特性、状态与边界 |
| [interaction.md](interaction.md) | 交互模式：无头设备三层远程通道（Web/SSH/Agent） |

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
| M10 | ext4 + BIO + Page Cache | 磁盘持久化 |
| M11 | 动态链接 + futex + TLS + 工具链 | 动态程序 + Go/Rust/C++ 交叉编译 |
| M12 | 设备 + Capabilities + Seccomp | Docker 安全模型 |
| M13 | 完整 /proc + 信号 + 事件 fd | 动态程序可观测性 |
| M14 | OCI 镜像 + 轻量运行时 + OTA（Redis/SQLite） | full 模式 ≤40MB 达标 |
| M15 | ARM64/RISC-V 评估 + 功能级补强 | aarch64 原型 + 电源/Flash/调试 |

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

full 模式的定位是**跑 musl 容器服务（Redis/SQLite 等交叉编译镜像）**，不是"任意 Docker 镜像"。需要实现以下内核扩展（见 [DESIGN.md §13](docs/DESIGN.md)）：
- **动态链接**——容器服务若动态链接，需要内核支持 PT_INTERP + MAP_SHARED（默认推荐静态编译）；
- **Capabilities + Seccomp**——容器安全模型依赖 Linux capability 位和 seccomp BPF 过滤；
- **devpts**——`docker exec` 需要 PTY（/dev/ptmx + /dev/pts/N）；
- **完整 /proc**——容器运行时读取 /proc/self/status 等路径；
- **veth/bridge**——容器网络默认模式（bridge）依赖 veth 对 + 二层转发。

mysql/kafka 等 **glibc 生态镜像不追**（glibc 兼容层价值递减、成本陡增，见 DESIGN §14 定位决策）。

**Q9：能跑 apt install 吗？**

降为 **P3 可选**。apt/dpkg 是 glibc 生态，与 musl 静态子集定位冲突；full 模式默认用交叉编译工具链分发软件，不依赖 apt。

**Q10：JVM 需要哪些特殊内核支持？**

JVM 需要动态链接 + futex + TLS + 信号 + getrandom + /proc/self/maps，且 OpenJDK 是 **glibc 构建**——musl 移植难度高。**降为 P3 可选**：先做 musl 构建可行性评估，不通过则不投入（避免掉进 glibc 陷阱）。

**Q11：Python 呢？比 JVM 简单吗？**

同属 P3 可选。Python（musl 版）需要动态链接 + getrandom + 基本信号，比 JVM 简单，但仍需评估 musl 构建；full 模式演示默认用 Redis/SQLite/Go 静态服务。
