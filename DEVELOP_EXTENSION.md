# 山水观心操作系统长期开发扩展（DEVELOP_EXTENSION）

> 本文档是 DEVELOPMENT.md 的**远期延伸**：v1.0（M0–M14）稳定交付后，沿**四条主线**长期演进。
> 设计依据见 [DESIGN_EXTENSION.md](DESIGN_EXTENSION.md)，数据结构级远期项见 [EXTENSIONS.md](EXTENSIONS.md)。
>
> 前三条主线不绑定固定里程碑，按**用户反馈优先级**择机启动；每项标注对应设计章节与依赖。
> **主线四（图形与桌面体验）**按 DESIGN_EXTENSION §4.1 的三阶段推进（v2.0+/v3.0/v4.0），
> 内存独立核算（≥128MB），与 Core 主线并行、互不阻塞。

---

## 主线一：契约式交付体系（致命 Bug → 免疫）

**触发条件（任一满足即启动）**：
- `shanshui-guanxin-check` 拦截日志中非 musl 二进制占比 > 5%；
- 用户反馈"安装软件困难"频次超过 3 次/月；
- 社区请求添加第三方软件源。

| 阶段 | 任务 | 设计依据 | 依赖 | 产出 | 验收标准 |
|---|---|---|---|---|---|
| 阶段一（MVP） | 官方推荐软件清单 + `shanshui-guanxin-build`（软件/功能/官网地址/官方 musl 静态二进制链接，core/runtime/service/net-tools 四类） | DESIGN §22.3 | M11 工具链 | 清单页 + `shanshui-guanxin-build` 工具 | 用户能按清单用 `shanshui-guanxin-build redis` 一键编译出可运行二进制 |
| 阶段一（立即可行） | 《为 山水观心操作系统构建 Redis》指南（Redis 7.2.4+，musl-gcc 静态编译） | DESIGN §22.4 | 无 | 指南文档 | 用户按指南 15 分钟内编译出可运行的 redis-server |
| 阶段二 | 社区软件仓库（`shanshui-guanxin repo-add`/`shanshui-guanxin install`；核心包：官网稳定源码 → musl-gcc 静态编译 → 打包 + 私钥签名 → 上传） | DESIGN §22.3 | 阶段一 | 软件仓库服务器 + 包维护流程 | `shanshui-guanxin install redis` 从官方源下载、验签、安装成功 |
| 阶段三 | 云端应用商店（`shanshui-guanxin deploy redis` 自动拉取最新最安全镜像部署） | DESIGN §22.3 | 云端构建（主线二） | 云端原生应用商店 | 浏览器点"部署 Redis"，设备自动拉取运行 |
| 并行 | `shanshui-guanxin-check` 升级适配器（glibc→musl syscall 翻译层 / 容器化沙盒评估） | DESIGN_EXT §1.1 | M11 `shanshui-guanxin-check` | 兼容性沙盒原型 | glibc 编译的 hello world 在沙盒内运行成功 |
| 并行 | Ext4 自适应挂载（检测 data=ordered，后台自动转 data=journal，不拒绝挂载） | DESIGN_EXT §1.2 | M10 ext4 | 挂载不拒绝、自动转换 | 现有 Ext4 盘直接挂载成功，转换进度可见 |
| 并行 | F2FS 支持评估（SD/eMMC 断电恢复 + 磨损均衡） | DESIGN_EXT §1.2 | M10 Block I/O | F2FS 可行性报告 | 在 SD 卡上 F2FS 读写测试通过，断电后 fsck 通过 |
| 并行 | Cgroup 内核级进程识别（内核识别 redis-server，自动应用安全策略） | DESIGN_EXT §1.3 | M6 cgroup | 内核强制限额，无视用户传参 | `redis-server --maxmemory 999gb` 在容器内仍被 Cgroup 限额截断 |

---

## 主线二：透明化体验（核心困惑 → 透明）

**触发条件（任一满足即启动）**：
- 用户反馈"交叉编译环境搭建困难"频次超过 5 次/月；
- `/etc/motd` 提示后仍有用户在设备上执行 `go build`；
- SMP v2.0 发布后，用户不知道如何利用多核。

| 阶段 | 任务 | 设计依据 | 依赖 | 产出 | 验收标准 |
|---|---|---|---|---|---|
| 阶段一 | 云端构建服务（浏览器写码 → 交叉编译 → OCI → OTA） | DESIGN_EXT §2.1 | M11 工具链 + M14 OTA | 云端 IDE/CI-CD 流水线 | 用户在浏览器提交代码，5 分钟内设备收到新容器 |
| 阶段一（并行） | `shanshui-guanxin-build` 一条命令源码 → 镜像 + 自动 `shanshui-guanxin-check` 验证 | DESIGN_EXT §2.1 | M11 工具链 | CLI 构建工具 | `shanshui-guanxin-build ./myapp` 输出 .shanshui-guanxin.tar，OTA 推设备运行 |
| 阶段二 | SMP 透明调度（默认跨核负载均衡） | DESIGN_EXT §2.2 | DESIGN §11 SMP | v2.0 多核调度 | 压测显示 2 核利用率 > 150%（相对 UP） |
| 阶段二（并行） | Cgroup CPU 亲和性（`shanshui-guanxin run --cpuset 0-1` 容器绑定核心） | DESIGN_EXT §2.2 | M6 cgroup + SMP | 延迟敏感场景绑定 | 工业 Modbus 容器绑定 CPU0，延迟抖动 < 5% |
| 阶段三 | Balena 等平台集成（设备管理/OTA） | DESIGN_EXT §2.3 | M14 OCI/OTA | 企业级设备管理适配 | Balena 仪表板显示 山水观心操作系统设备在线，可远程部署容器 |
| 阶段三（并行） | `shanshui-guanxin` CLI 标准化（事实标准构建工具） | DESIGN_EXT §2.3 | 全部 CLI 能力 | 统一命令行规范 | 社区项目主动提供 shanshui-guanxin-build 配置模板 |

---

## 主线三：深度定制（高频需求 → 好用）

**触发条件（任一满足即启动）**：
- 工业用户明确提出 Modbus 响应延迟 > 100ms；
- 运维团队反馈排查问题需重启设备查看日志；
- 用户要求系统升级支持断电安全。

| 阶段 | 任务 | 设计依据 | 依赖 | 产出 | 验收标准 |
|---|---|---|---|---|---|
| 阶段一 | SCHED_FIFO/RR 完整实现（POSIX 实时调度类） | DESIGN_EXT §3.1 | M2 RT 预留 + M9 | 硬实时任务调度 | Modbus 采集任务 100 次响应，最大抖动 < 2ms |
| 阶段一（并行） | 中断线程化（Threaded Interrupts） | DESIGN_EXT §3.1 | SMP | 减少关中断时间 | 中断关闭时间从 50μs 降至 10μs |
| 阶段二 | 极简 eBPF 子集（动态探针 + 性能计数） | DESIGN_EXT §3.2 | M13 packet_trace | 动态追踪替代日志 | `shanshui-guanxin trace --func tcp_rcv` 输出实时调用，不重启内核 |
| 阶段二（并行） | 结构化日志（JSON，对接 ELK） | DESIGN_EXT §3.2 | M9 可观测性 | 远程聚合分析 | `/var/log/kernel.json` 可被 Filebeat 采集，Kibana 展示 |
| 阶段二（并行） | top 系统监控 + GUI 版系统监视器 | DESIGN_EXT §3.2 | M9 top | 本地多端监控 | `top` 输出与 `/proc` 一致 |
| 阶段二（并行） | **`/top` HTTP 接口**：经 shanshui-guanxin-gateway 暴露，返回 JSON（进程/CPU/内存数据），供 Web 前端展示 | DESIGN_EXT §3.2 | M14 gateway | Web 监控接口 | 浏览器访问 `/top` 返回合法 JSON，与 `top` 数据一致 |
| 阶段三 | A/B 根文件系统分区（系统层原子升级） | DESIGN_EXT §3.3 | M14 OTA | 内核升级断电不变砖 | 内核升级过程中随机断电，重启后自动回滚或继续完成升级 |
| 阶段三（并行） | 系统配置不可变性（只读镜像 + `/var`/`/data` 可写） | DESIGN_EXT §3.3 | M14 OTA + overlayfs | Ubuntu Core 式定制 | `echo "test" > /etc/hostname` 失败（只读），`echo "test" > /var/conf/hostname` 成功覆盖 |

---

## 社区驱动需求（硬件连接性 / 服务可靠性 / 远程可运维性）

> 由真实用户画像直接驱动，按紧迫度分三档，设计见 DESIGN §23 / DESIGN_EXTENSION §3.4。
> 全部**只增不减**；v1.0（M0–M14）后按反馈优先级启动。

### 基础生存型（v1.1）

| 任务 | 设计依据 | 依赖 | 产出 |
|---|---|---|---|
| 容器保活策略（`restartPolicy`: always/on-failure/unless-stopped） | DESIGN §23.1 | M3 init | OCI spec 扩展字段 + 自愈 |
| shanshui-guanxin-gateway（HTTP/1.1 + TLS + 反向代理，承载 Web UI） | DESIGN_EXT §3.5 | M5 HTTP / M14 | `shanshui-guanxin gateway` 一条命令启动 |
| Web 管理界面默认开启（`shanshui-guanxin-webui` 端口 80） | DESIGN §23.1 | M5 HTTP / M14 | 按钮化容器管理 |

### 基础生存型（v1.5）

| 任务 | 设计依据 | 依赖 | 产出 |
|---|---|---|---|
| 4G/5G 蜂窝上网（PPP + USB 串口 + wpa_supplicant） | DESIGN §23.1 | USB Host / UART | 蜂窝/Wi-Fi 联网 |
| 持久化日志（`/var/log/journal/` + 轮转 + 总大小限制） | DESIGN §23.1 | M10 ext4 | 重启后日志可回溯 |
| WireGuard VPN（`shanshui-guanxin-vpn`） | DESIGN §23.1 | M5 网络栈 | 站点到站点安全连接 |
| 存储卷独占锁（`--volume-exclusive`，flock/leases） | DESIGN §23.1 | M13 记录锁 | SQLite 并发写安全 |
| MicroPython 运行时（<256KB，官方镜像） | DESIGN §23.1 | M14 仓库 | 脚本数据清洗 |
| PTP/NTP 精确时间同步（chrony 轻量版 + PTP 评估） | DESIGN §23.1 | M5 SNTP | 多设备时间戳一致 |

### 进阶生产力型（v2.0+）

| 任务 | 设计依据 | 依赖 | 产出 |
|---|---|---|---|
| 四层负载均衡 L4LB（IPVS 轮询） | DESIGN §23.2 | 网关架构升级 | 多容器流量分发 |
| 流量镜像（灰度验证） | DESIGN §23.2 | 网关转发层 | 镜像端口 |

---

## 主线四：图形与桌面体验（命令行 → 可视交互）

**触发条件（任一满足即启动）**：
- 工业/消费用户提出需要本地可视化操作；
- 用户要求在设备上直接查看监控面板（而非仅 Web）；
- 社区请求提供图形化配置工具。

> 与 DEVELOPMENT.md **M16（Desktop 图形版）** 对应：M16 是首个里程碑，本主线按 DESIGN_EXTENSION §4.1
> 三阶段长期推进；内存独立核算（≥128MB），不适用 32MB/40MB 断言。

| 阶段 | 子任务（对应 DEVELOPMENT M16） | 设计依据 | 依赖 | 产出 | 验收标准（含内存） |
|---|---|---|---|---|---|
| 阶段一（v2.0+） | M16-1 fbdev 框架（`/dev/fb*` + `/dev/tty0` + FBIOGET/PUT_VSCREENINFO + DMA 线性帧缓冲 mmap） | DESIGN §13.16 | M12 设备框架 | 最小显示输出 | QEMU `-vga virtio` 截图可见测试图案；内核增量 ≤16MB |
| 阶段一 | M16-2 DRM KMS 最小子集（drm 核心注册、`crtc/page_flip`、VBlank 中断） | DESIGN §6.2⑤、§13.16 | M16-1 | KMS 接口 | 页翻转无撕裂；VBlank 计数正确 |
| 阶段二（v3.0） | M16-3 Wayland 合成器（`wl_display`/`wl_compositor`/`wl_surface`/`wl_shell`，两窗口渲染） | DESIGN §13.16 | M16-2 | 可叠加窗口 | 两窗口叠加渲染、可拖动缩放；用户态图形栈 20–40MB |
| 阶段二 | M16-4 图形库（egui/fltk-rs）+ shanshui-guanxin-monitor（与 top 共享 `/proc` 数据源） | DESIGN §13.16、§10.2 | M16-3 + M9 top | 可视化监视 | CPU/内存曲线实时刷新，与 top 数据一致 |
| 阶段三（v4.0） | M16-5 shanshui-guanxin-fm 文件管理器（"文档/下载/桌面" + 盘符 C:/D: 映射 + 复制/移动/删除） | DESIGN §3.6、§19.3、§20.1 | M16-4 | 桌面应用 | 盘符视图与快捷入口可用；总内存 128–256MB |

---

## 演进节奏（与 VERSIONING 对齐）

| 阶段 | 版本 | 主线重点 |
|---|---|---|
| v1.0 | minimal/full 稳定 | 三主线全部停留在"现状"层（DESIGN §21 的拦截/提示/预留） |
| v2.0 | full + SMP | 主线二（云端构建、SMP 透明调度）+ 主线三启动（SCHED_FIFO、结构化日志）+ **主线四阶段一（帧缓冲/DRM）** |
| v3.0+ | 产品级 | 主线一（应用商店、FS 自动转换、Cgroup 硬化）+ 主线三深水区（eBPF、A/B 原子升级）+ **主线四阶段二（Wayland 合成器）** |
| v4.0+ | Desktop 正式版 | **主线四阶段三（完整桌面环境）** |

> 主线一的"免疫"依赖生态成熟（仓库/签名/适配器），节奏最慢；主线三的"好用"直接决定
> 工业用户是否买单，节奏最快；主线二贯穿全程，是获客与留存的底层能力；主线四独立核算内存，
> 由图形化需求触发，与其余主线并行。
> 社区驱动需求穿插在 v1.1–v2.0+，由真实用户反馈优先级决定启动顺序，与四主线并行推进。
