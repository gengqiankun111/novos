# Novos-OS 长期开发扩展（DEVELOP_EXTENSION）

> 本文档是 DEVELOPMENT.md 的**远期延伸**：v1.0（M0–M14）稳定交付后，沿三条主线长期演进。
> 设计依据见 [DESIGN_EXTENSION.md](DESIGN_EXTENSION.md)，数据结构级远期项见 [EXTENSIONS.md](EXTENSIONS.md)。
>
> 三条主线不绑定固定里程碑，按**用户反馈优先级**择机启动；每项标注对应设计章节与依赖。

---

## 主线一：契约式交付体系（致命 Bug → 免疫）

**触发条件（任一满足即启动）**：
- `novos-check` 拦截日志中非 musl 二进制占比 > 5%；
- 用户反馈"安装软件困难"频次超过 3 次/月；
- 社区请求添加第三方软件源。

| 阶段 | 任务 | 设计依据 | 依赖 | 产出 | 验收标准 |
|---|---|---|---|---|---|
| 阶段一（MVP） | 官方推荐软件清单 + `novos-build`（软件/功能/官网地址/官方 musl 静态二进制链接，core/runtime/service/net-tools 四类） | DESIGN §22.3 | M11 工具链 | 清单页 + `novos-build` 工具 | 用户能按清单用 `novos-build redis` 一键编译出可运行二进制 |
| 阶段一（立即可行） | 《为 Novos-OS 构建 Redis》指南（Redis 7.2.4+，musl-gcc 静态编译） | DESIGN §22.4 | 无 | 指南文档 | 用户按指南 15 分钟内编译出可运行的 redis-server |
| 阶段二 | 社区软件仓库（`novos repo-add`/`novos install`；核心包：官网稳定源码 → musl-gcc 静态编译 → 打包 + 私钥签名 → 上传） | DESIGN §22.3 | 阶段一 | 软件仓库服务器 + 包维护流程 | `novos install redis` 从官方源下载、验签、安装成功 |
| 阶段三 | 云端应用商店（`novos deploy redis` 自动拉取最新最安全镜像部署） | DESIGN §22.3 | 云端构建（主线二） | 云端原生应用商店 | 浏览器点"部署 Redis"，设备自动拉取运行 |
| 并行 | `novos-check` 升级适配器（glibc→musl syscall 翻译层 / 容器化沙盒评估） | DESIGN_EXT §1.1 | M11 `novos-check` | 兼容性沙盒原型 | glibc 编译的 hello world 在沙盒内运行成功 |
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
| 阶段一（并行） | `novos-build` 一条命令源码 → 镜像 + 自动 `novos-check` 验证 | DESIGN_EXT §2.1 | M11 工具链 | CLI 构建工具 | `novos-build ./myapp` 输出 .novos.tar，OTA 推设备运行 |
| 阶段二 | SMP 透明调度（默认跨核负载均衡） | DESIGN_EXT §2.2 | DESIGN §11 SMP | v2.0 多核调度 | 压测显示 2 核利用率 > 150%（相对 UP） |
| 阶段二（并行） | Cgroup CPU 亲和性（`novos run --cpuset 0-1` 容器绑定核心） | DESIGN_EXT §2.2 | M6 cgroup + SMP | 延迟敏感场景绑定 | 工业 Modbus 容器绑定 CPU0，延迟抖动 < 5% |
| 阶段三 | Balena 等平台集成（设备管理/OTA） | DESIGN_EXT §2.3 | M14 OCI/OTA | 企业级设备管理适配 | Balena 仪表板显示 Novos 设备在线，可远程部署容器 |
| 阶段三（并行） | `novos` CLI 标准化（事实标准构建工具） | DESIGN_EXT §2.3 | 全部 CLI 能力 | 统一命令行规范 | 社区项目主动提供 novos-build 配置模板 |

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
| 阶段二 | 极简 eBPF 子集（动态探针 + 性能计数） | DESIGN_EXT §3.2 | M13 packet_trace | 动态追踪替代日志 | `novos trace --func tcp_rcv` 输出实时调用，不重启内核 |
| 阶段二（并行） | 结构化日志（JSON，对接 ELK） | DESIGN_EXT §3.2 | M9 可观测性 | 远程聚合分析 | `/var/log/kernel.json` 可被 Filebeat 采集，Kibana 展示 |
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
| Web 管理界面默认开启（`novos-webui` 端口 80） | DESIGN §23.1 | M5 HTTP / M14 | 按钮化容器管理 |

### 基础生存型（v1.5）

| 任务 | 设计依据 | 依赖 | 产出 |
|---|---|---|---|
| 4G/5G 蜂窝上网（PPP + USB 串口 + wpa_supplicant） | DESIGN §23.1 | USB Host / UART | 蜂窝/Wi-Fi 联网 |
| 持久化日志（`/var/log/journal/` + 轮转 + 总大小限制） | DESIGN §23.1 | M10 ext4 | 重启后日志可回溯 |
| WireGuard VPN（`novos-vpn`） | DESIGN §23.1 | M5 网络栈 | 站点到站点安全连接 |
| 存储卷独占锁（`--volume-exclusive`，flock/leases） | DESIGN §23.1 | M13 记录锁 | SQLite 并发写安全 |
| MicroPython 运行时（<256KB，官方镜像） | DESIGN §23.1 | M14 仓库 | 脚本数据清洗 |
| PTP/NTP 精确时间同步（chrony 轻量版 + PTP 评估） | DESIGN §23.1 | M5 SNTP | 多设备时间戳一致 |

### 进阶生产力型（v2.0+）

| 任务 | 设计依据 | 依赖 | 产出 |
|---|---|---|---|
| 四层负载均衡 L4LB（IPVS 轮询） | DESIGN §23.2 | 网关架构升级 | 多容器流量分发 |
| 流量镜像（灰度验证） | DESIGN §23.2 | 网关转发层 | 镜像端口 |

---

## 演进节奏（与 VERSIONING 对齐）

| 阶段 | 版本 | 主线重点 |
|---|---|---|
| v1.0 | minimal/full 稳定 | 三主线全部停留在"现状"层（DESIGN §21 的拦截/提示/预留） |
| v2.0 | full + SMP | 主线二（云端构建、SMP 透明调度）+ 主线三启动（SCHED_FIFO、结构化日志） |
| v3.0+ | 产品级 | 主线一（应用商店、FS 自动转换、Cgroup 硬化）+ 主线三深水区（eBPF、A/B 原子升级） |

> 主线一的"免疫"依赖生态成熟（仓库/签名/适配器），节奏最慢；主线三的"好用"直接决定
> 工业用户是否买单，节奏最快；主线二贯穿全程，是获客与留存的底层能力。
> 社区驱动需求穿插在 v1.1–v2.0+，由真实用户反馈优先级决定启动顺序，与三主线并行推进。
