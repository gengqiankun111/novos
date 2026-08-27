# 山水观心操作系统 full 模式开发任务清单（M10–M14）

> 本文档把 `DEVELOPMENT.md` 的 M10–M14 拆解到可执行任务级别，给出优先级排序、依赖关系、工作量估算与验收标准。
> 与 `DESIGN.md` §13 的扩展点一一对应，任务编号中的引用（§13.x）指向设计文档。

## 0. 优先级体系与任务编号

| 级别 | 含义 | 处理规则 |
|---|---|---|
| **P0** | 阻塞级 | 处于关键路径或被其他任务依赖，最先启动，不做完不进入下一阶段 |
| **P1** | 必备 | 里程碑目标必需，在 P0 完成后立即跟进 |
| **P2** | 后移可 | 可推迟到里程碑末段，不影响主线验收 |
| **P3** | 可选 | 增强项，不阻塞任何目标，按余力决定 |

任务编号规则：`M<里程碑>-<序号>`（如 `M10-03`）。工作量单位为**人日**（单人全力 8 小时），估算含编码、单测、调试。

状态标记：`[ ]` 未开始 / `[~]` 进行中 / `[x]` 完成。

---

## 1. 关键路径与并行策略

```
主关键路径（串行主线）
────────────────────────────────────────────────────────────
M10-01 virtio-blk → M10-02 BIO → M10-03 Page Cache
  → M10-04/05 ext4 解析 → M10-08 MAP_SHARED
  → M11-01/02/03 动态加载 → M11-05 futex → M11-07/08/09 TLS
  → M13-01 maps → M13-07/09 信号（JVM 依赖）
  → M14-01/02 veth/bridge → M14-04 OCI → M14-13 端到端验收

并行支线（只依赖 M9，可与主线并行）
────────────────────────────────────────────────────────────
M12 全部：devtmpfs / 设备 / Capabilities / Seccomp / getrandom

主线外挂（依赖主线某些节点，可在对应节点后跟进）
────────────────────────────────────────────────────────────
M14-06 containerd / M14-07 image pull → M14-08 apt / M14-10 JVM / M14-11 Python
```

两条路线建议的团队分工：路线 A（M10→M11→M13→M14 主线）与路线 B（M12 支线）互不阻塞；若只有一人，先走主线，M12 在主线等待 M10 集成测试的空档插入。

---

## 2. M10：Block I/O + ext4 + Page Cache（目标 ≤36MB）

本里程碑为磁盘文件系统打地基。ext4 驱动的正确性是后续持久化的前提，**写入路径实现 `data=journal` 完整模式**（2026-08 评审修正：有序写无 journal 时掉电几乎必毁元数据；journal buffer 约占内存 5–10%，计入预算台账，§13.3）。

| 编号 | 任务 | 优先级 | 依赖 | 工作量 | 验收标准 |
|---|---|---|---|---|---|
| M10-01 | virtio‑blk 驱动：PCI 探测、split descriptor ring、`read_block/write_block/flush` | **P0** | M9 | 6 | QEMU 内 `virtio-blk` 设备枚举成功，读写块与宿主镜像一致 |
| M10-02 | BIO 层：`Bio` 结构 + 简单队列 + 同步/异步完成路径 | **P0** | M10-01 | 3 | 单元测试覆盖 read/write/flush 三种 op，队列无泄漏 |
| M10-03 | Page Cache：`AddressSpace`（文件偏移→物理页）、`readpage/writepage`、脏页跟踪 | **P0** | M10-02 | 6 | 两次 `readpage` 同一偏移命中同一物理页；脏页可被标记与回写 |
| M10-04 | ext4 超级块 + inode 表解析（`FileSystemDriver` trait 实现） | **P0** | M10-02 | 8 | 挂载 ext4 镜像后能列出根目录 inode，块组描述符解析正确 |
| M10-05 | ext4 目录项 + extent 树解析（lookup/create/unlink） | **P0** | M10-04 | 8 | `ls`、`mkdir`、`touch`、`rm` 在 ext4 上与 tmpfs 行为一致 |
| M10-06 | ext4 写入路径：块分配 + **data=journal 完整模式**（事务/journal 日志/检查点） | **P1** | M10-05 | 12 | 写 10MB 文件后提交事务，断电模拟（QEMU 随机 kill）后元数据一致 |
| M10-07 | `mount` 系统调用扩展：`mount -t ext4 /dev/vda /mnt` | **P1** | M10-05 | 2 | mount 后路径解析正确切换 SuperBlock，umount 释放资源 |
| M10-08 | `mmap MAP_SHARED` 文件映射：走 Page Cache，多进程共享物理页 | **P0** | M10-03 | 5 | 两进程映射同一 .so，`/proc/self/maps` 显示相同 PFN（或 page cache 命中计数验证） |
| M10-09 | Page Cache shrink：脏页回写 + 逐出 + 计入 `MemStat` | **P1** | M10-03, M10-06 | 3 | 填充 16MB 文件缓存后触发 shrink，`page_cache_bytes` 回落到目标水位 |
| M10-10 | 集成测试：ext4 读写/删除/持久化 + 重启恢复断言 | **P1** | M10-06 | 3 | QEMU 内写文件 → 重启 → 文件仍在且内容一致 |
| M10-11 | 内存基线测量与回填（≤36MB） | **P1** | M10-09 | 1 | `docs/bench/<date>/` 记录实测数据，CI 断言通过 |

---

## 3. M11：动态链接 + futex + TLS（目标 ≤38MB）

本里程碑让内核能运行 musl 动态链接的 ELF，并补齐 pthread 的两大底层依赖（futex 与 TLS）。MAP_SHARED 复用 M10-08，不重复实现。

| 编号 | 任务 | 优先级 | 依赖 | 工作量 | 验收标准 |
|---|---|---|---|---|---|
| M11-01 | ELF 加载器扩展：解析 `PT_INTERP` + `PT_DYNAMIC` + `DT_NEEDED` | **P0** | M10-08 | 4 | 单元测试覆盖静态/动态/无 interp 三种 ELF 的判定与解析 |
| M11-02 | 辅助向量设置：`AT_BASE/AT_PHDR/AT_PHENT/AT_PHNUM/AT_RANDOM/AT_EXECFN` | **P0** | M11-01 | 2 | `getauxval` 在用户态读到正确的值（musl 自检通过） |
| M11-03 | 内核加载 `ld-musl-x86_64.so.1` 到地址空间，跳转其入口 | **P0** | M11-01, M11-02 | 4 | 动态链接 hello world 启动并输出（不依赖内核解析 GOT/PLT） |
| M11-04 | futex 系统调用：`WAIT/WAKE`，按物理页地址索引等待队列 | **P0** | M9 | 4 | 两线程互斥锁（pthread_mutex）正确互斥，无丢失唤醒 |
| M11-05 | futex 扩展：`REQUEUE` + 超时参数（`FUTEX_WAIT_BITSET` 可选） | **P1** | M11-04 | 2 | 条件变量（pthread_cond）唤醒无惊群；超时返回 ETIMEDOUT |
| M11-06 | `arch_prctl(ARCH_SET_FS/GET_FS)` + FS base MSR 写入 | **P0** | M9 | 3 | 用户态 `%fs` 访问 TLS 区数据正确 |
| M11-07 | 上下文切换恢复 FS base（每个 Task 保存/恢复） | **P0** | M11-06 | 2 | 多线程切换后各自 TLS 数据不串扰 |
| M11-08 | clone 扩展：`CLONE_SETTLS` + `CLONE_CHILD_CLEARTID` | **P0** | M11-07 | 3 | `pthread_create` 创建 100 线程无泄漏，退出后 tid 地址被清零 |
| M11-09 | 移植 musl 动态链接用户态二进制（libc.so、ld.so、busybox 动态版） | **P1** | M11-03 | 3 | 动态 busybox 的 `ls/cat/echo` 全部可用 |
| M11-10 | pthread 集成测试：mutex/cond/thread 生命周期 | **P1** | M11-05, M11-08 | 3 | `pthread` 压测 1000 次创建/销毁无死锁、无内存增长 |
| M11-11 | 内存基线测量与回填（≤38MB） | **P1** | 全部 | 1 | 动态链接进程常驻统计正确，CI 断言通过 |
| M11-12 | 交叉工具链 + ABI 契约（§15.2）：musl-cross + crt1/crti/crtn + linker script + 版本锁定（宿主机侧）；`docs/abi.md`（syscall **白/黑/灰名单**/结构体/errno/调用约定） | **P1** | M11-03 | 10 | 工具链出静态二进制可在 山水观心操作系统加载；abi.md 覆盖 musl syscall 足迹并含黑白名单 |
| M11-13 | **Novos-SDK 基础镜像**：ld-musl + 头文件 + linker script；第三方应用强制 `--dynamic-linker` 指向 `/novos/` 路径（§15.2） | **P1** | M11-12 | 4 | 动态链接应用经 SDK 构建后不在宿主 syscall 上爆炸 |
| M11-14 | **novos-check 工具**：ELF syscall 依赖扫描 + 内存足迹预估（RSS+虚拟内存）（§15.3） | **P1** | M11-12 | 5 | 对 Redis/busybox 扫描输出 syscall 清单与内存预估；白名单外 syscall 报警 |
| M11-15 | 宿主机交叉编译冒烟：Go（`CGO_ENABLED=0`）、Rust（`x86_64-unknown-linux-musl`）、C++（`-static -static-libstdc++ -static-libgcc`），产物 OTA 下发 | **P1** | M11-12 | 7 | 三语言静态二进制在 山水观心操作系统上运行输出；CI 断言通过 |

---

## 4. M12：设备框架 + Capabilities + Seccomp（目标 ≤39MB）

本里程碑只依赖 M9，与 M10/M11 完全并行。为 Docker 安全模型与设备文件补齐基础。

| 编号 | 任务 | 优先级 | 依赖 | 工作量 | 验收标准 |
|---|---|---|---|---|---|
| M12-01 | `CharDevice` trait + devtmpfs 挂载（自动注册设备节点） | **P0** | M9 | 3 | `/dev` 目录存在，设备节点按注册表自动出现 |
| M12-02 | 标准设备：`/dev/null`、`/dev/zero` | **P1** | M12-01 | 1 | `echo hi > /dev/null` 无输出无报错；`dd if=/dev/zero` 返回全零 |
| M12-03 | 熵源：RDRAND 封装 + 混合熵（jiffies/PID/网络时间戳） | **P0** | M9 | 2 | 连续读取无重复模式，熵池计数单调 |
| M12-04 | `getrandom` 系统调用 + `/dev/urandom` | **P0** | M12-03 | 2 | `getrandom(2)` 返回随机字节；Python `os.urandom` 可用 |
| M12-05 | devpts：`/dev/ptmx` + `/dev/pts/N`（PTY 对 + 环形缓冲 + 行编辑） | **P1** | M12-01 | 6 | 打开 ptmx → 写 → pts 读到；`stty` 基本 ioctl 生效 |
| M12-06 | ioctl 框架扩展：`TIOCSPGRP/TIOCGWINSZ/TCGETS` 等 | **P2** | M12-05 | 3 | shell 交互（信号控制、窗口大小查询）在 PTY 下正常 |
| M12-07 | Capabilities：`TaskCreds`（permitted/effective/inheritable/bounding） | **P0** | M9 | 4 | creds 结构正确；root 与非 root 检查路径分离 |
| M12-08 | 权限检查改造：`check_cap` 接入 mount/pivot_root/bind/raw socket 等路径 | **P0** | M12-07 | 3 | 无 CAP_SYS_ADMIN 的进程 mount 返回 EPERM |
| M12-09 | Capabilities 继承/bounding：exec 时按 inheritable+bounding 收敛 | **P1** | M12-08 | 2 | setuid 之外的 exec 后 caps 正确收敛（不放大） |
| M12-10 | Seccomp BPF 最小解释器（syscall number 过滤，<500 行） | **P0** | M9 | 4 | 单元测试覆盖 ALLOW/KILL/ERRNO 三动作 + 默认动作 |
| M12-11 | seccomp syscall：`prctl(PR_SET_SECCOMP)` + filter 装载 + Task 关联 | **P0** | M12-10 | 2 | 容器内调 `reboot` 被 filter 拦截返回 EPERM |
| M12-12 | Docker 默认 seccomp profile 兼容性测试 | **P2** | M12-11 | 3 | 用 Docker 默认 profile 跑 busybox 全命令集，无被误杀 |
| M12-13 | 内存基线测量与回填（≤39MB） | **P1** | 全部 | 1 | CI 断言通过 |

---

## 5. M13：完整 /proc + 信号扩展 + 事件 fd（目标 ≤39MB）

本里程碑的核心验收是 JVM 能正常启动。`/proc/self/maps` 与 `sigaltstack` 是 JVM 的硬依赖，列为 P0。

| 编号 | 任务 | 优先级 | 依赖 | 工作量 | 验收标准 |
|---|---|---|---|---|---|
| M13-01 | `/proc/self/maps`：地址空间遍历 + 权限位 + 文件/匿名标注 | **P0** | M11-01 | 4 | 格式与 Linux 一致；JVM 启动后 maps 列出全部段（text/so/stack/vdso） |
| M13-02 | `/proc/self/status`：`VmRSS/VmPeak/Threads/Uid/Gid` | **P1** | M13-01 | 2 | JVM `Runtime.getRuntime().totalMemory` 与 status 的 RSS 量级一致 |
| M13-03 | `/proc/self/exe`：符号链接到可执行文件 | **P1** | M13-01 | 2 | `readlink /proc/self/exe` 返回正确路径；JVM 用它定位 java.home |
| M13-04 | `/proc/self/fd/` 目录 + `/proc/<pid>/` 视图（maps/status/exe/fd/cmdline） | **P2** | M13-02, M13-03 | 5 | `ls /proc/self/fd` 列出 fd；`/proc/<pid>` 对非 self 进程可读（同 uid） |
| M13-05 | `/proc/cpuinfo`、`/proc/mounts`、`/proc/filesystems` | **P2** | M13-01 | 2 | JVM 能读到 `cpuinfo` 型号与特性标志；Docker 检查 mounts/filesystems 通过 |
| M13-06 | sigaction 扩展：`SA_SIGINFO/SA_ONSTACK/SA_RESTART/SA_NODEFER` | **P0** | M9 | 3 | 注册 SA_SIGINFO handler 后，信号帧携带 siginfo 正确字段 |
| M13-07 | sigaltstack：`sigaltstack` syscall + 信号投递到备用栈 | **P0** | M13-06 | 3 | JVM 在 altstack 上处理 SIGSEGV 不溢出主栈 |
| M13-08 | sigprocmask + 信号扩展到 64（实时信号 33–63） | **P1** | M13-06 | 2 | 阻塞/解除阻塞语义正确；实时信号排队不丢失 |
| M13-09 | sigreturn 恢复增强：完整恢复 sigframe（含 altstack 状态） | **P1** | M13-06, M13-07 | 2 | handler 返回后栈/寄存器/信号掩码完整恢复，连续触发 1 万次无累积 |
| M13-10 | SIGSEGV 正确投递路径：用户态非法访问 → SIGSEGV（非内核 panic） | **P0** | M13-06 | 2 | 试探写 null 地址的程序收到 SIGSEGV，内核不 panic |
| M13-11 | timerfd：`timerfd_create` + `timerfd_settime` + epoll 可监听 | **P1** | M9 | 3 | epoll 监听 timerfd，到期返回 EPOLLIN，读回超时计数 |
| M13-12 | signalfd：`signalfd` syscall（可选） | **P3** | M13-06 | 3 | 事件循环库（libevent 风格）可用 signalfd 收信号 |
| M13-13 | JVM 冒烟测试：启动 OpenJDK 最小镜像跑 `System.out.println` | **P1** | M13-01/07/10 | 3 | JVM 完成启动、类加载、打印、退出，无崩溃 |
| M13-14 | 内存基线测量与回填（≤39MB） | **P1** | 全部 | 1 | CI 断言通过 |

---

## 6. M14：OCI 镜像 + 轻量容器运行时（OTA 升级回滚）（目标 ≤40MB）

> 定位调整（DESIGN.md §14/§16）：**"支持 Docker" 重述为 "支持 OCI 镜像 + 轻量容器运行时（含 OTA）"**，不做 docker daemon/CLI 兼容；`apt/JVM/Python` 属 glibc 生态，降为 P3 可选；full 模式以"跑真实 musl 容器服务 + OTA 可演示"为验收锚点。

本里程碑把全部扩展点收拢成端到端能力。veth/bridge 与 OCI 解析是容器网络和容器创建的前置。

| 编号 | 任务 | 优先级 | 依赖 | 工作量 | 验收标准 |
|---|---|---|---|---|---|
| M14-01 | veth pair：虚拟设备对（容器侧 + 宿主侧），挂入 net namespace | **P0** | M9 | 6 | 容器内 ping 宿主 bridge IP 通；两端设备计数正确 |
| M14-02 | bridge：MAC 学习 + 二层转发 | **P0** | M14-01 | 6 | 两容器互 ping 通；bridge 转发表随流量更新 |
| M14-03 | DNAT 完整端口映射：`-p 8080:80` 语义（外部访问容器服务） | **P1** | M14-02 | 3 | 宿主 8080 端口 → 容器 80，conntrack 反向还原正确 |
| M14-04 | OCI runtime spec 解析：`config.json`（rootfs/mounts/capabilities/seccomp/env） | **P0** | M9 | 5 | 解析 busybox OCI bundle 并生成正确的容器创建参数 |
| M14-05 | 容器创建流程接入扩展：seccomp filter + capabilities + devpts 挂载 | **P1** | M14-04, M12-07/10 | 3 | 容器内进程的 cap 集与 seccomp profile 与 config.json 一致 |
| M14-06 | 轻量容器运行时：生命周期 + overlayfs 组装 + 状态存储（DESIGN §4.6，**不做 daemon 兼容**） | **P1** | M14-05 | 10 | `novos run/stop` 全流程无泄漏；状态可持久化 |
| M14-07 | `novos-pull`：registry HTTPS + OCI 解析 + SHA-256 摘要校验 + 层解压 | **P1** | M14-06 | 6 | 从 registry 拉 busybox/redis 镜像，摘要校验通过 |
| M14-08 | **最小记录锁**：`fcntl(F_SETLK/F_GETLK/F_UNLCK)` 字节区间锁（按文件组织锁表 `{owner,start,len,type}`） | **P0** | M10-05 | 3 | SQLite 并发读写不损坏库文件；`F_GETLK` 查询正确；锁随 fd 关闭/进程退出释放 |
| M14-09 | 移植 **SQLite**（musl 静态 `libsqlite3.a`，`SQLITE_THREADSAFE=0`）：CRUD + WAL | **P1** | M14-08, M11-03 | 6 | 容器内建表/增删改查通过；`PRAGMA journal_mode=WAL` 重启后数据持久化 |
| M14-10 | 移植 **Redis**（musl 编译）：`SET/GET`、AOF/RDB、外部 TCP 访问（**部署模板强制**：`--maxmemory 64mb --maxmemory-policy allkeys-lru`、禁 RDB `save ""`、只开 AOF+重写，§18.3） | **P1** | M14-06, M13-11 | 8 | 容器外 `redis-cli SET/GET` 经端口映射可访问；AOF 重启恢复；满内存时按 allkeys-lru 淘汰而非拒绝 |
| M14-11 | **OTA 升级 + 回滚**：层增量拉取 + 镜像版本切换（出错切回旧层） | **P0** | M14-07 | 8 | 更新镜像层 → 增量拉取 → 重启生效；回滚旧层可恢复；坏镜像不破坏运行态 |
| M14-12 | HTTPS/TLS 用户态库（mbedTLS 或 Rust TLS），`novos-pull` / P3 包管理走 HTTPS | **P2** | M14-07 | 5 | 从 registry 走 HTTPS 拉镜像成功 |
| M14-13 | 端到端验收：`novos run redis` + 容器内 SQLite + OTA 演示 + `novos run busybox` | **P0** | M14-09/10/11/06 | 4 | 四项演示命令全部通过；无内核 panic |
| M14-14 | 内存基线最终测量（≤40MB）与文档回填 | **P1** | M14-13 | 1 | full 模式 CI 断言通过，`docs/bench/` 回填 |
| M14-15 | （P3 可选）apt + dpkg 移植（动态链接 musl 版） | **P3** | M14-06, M11-03 | 6 | `apt update` 成功；小包安装后可执行 |
| M14-16 | （P3 可选）OpenJDK / CPython 移植（需先评估 musl 构建可行性） | **P3** | M13-13 | 12 | `java -version` / `python3 --version` 输出 |
| M14-17 | （值得）Mosquitto MQTT broker（musl 静态）：IoT 设备接入，Pub/Sub + QoS | **P2** | M14-10 | 5 | 设备经 MQTT 发布/订阅消息；与 Redis Pub-Sub 桥接可用 |
| M14-18 | （值得）Modbus 工业协议（采集侧，网关核心） | **P2** | M14-01 | 8 | Modbus RTU/TCP 读寄存器 → JSON 结构化 → 上报链路打通 |
| M14-19 | （值得）内置 Web 管理界面（轻量 HTTP + 静态前端） | **P2** | M14-12 | 6 | 浏览器访问设备 IP 可查看服务状态/配置/日志 |
| M14-20 | （值得）SSH（dropbear 轻量实现）：开发层远程调试/救援（§20.1） | **P2** | M12-05 | 4 | 终端 SSH 登入设备 shell；PTY 交互正常 |
| M14-21 | （值得）Agent 主动上联 + 离线导入（`docker save` tar → Web 上传/U 盘）（§20.2） | **P2** | M14-13 | 6 | Agent 回报状态/日志；离线 tar 导入后可运行镜像 |
| M14-22 | HTTP 客户端增强：SSE 流式响应 + 长超时 + 大 JSON 流式解析（AI 调用 = HTTP 用例，DESIGN §18.6） | **P2** | M14-12 | 4 | 流式拉取云端 API 响应不整包缓冲 |
| M14-23 | （值得）Lua / MicroPython / QuickJS 轻量脚本运行时 | **P3** | M11-13 | 6 | 脚本容器内运行 hello 级验证 |

---

## 7. 推荐执行顺序（跨里程碑排序）

按优先级从高到低、依赖先后排列。P0 任务按关键路径顺序执行；P1/P2 在其所属里程碑内跟进；M12 支线可与主线任意节点并行。

| 序号 | 任务 | 所属 | 级别 | 说明 |
|---|---|---|---|---|
| 1 | M10-01 | M10 | P0 | 全链路地基，先于一切 |
| 2 | M10-02 | M10 | P0 | BIO 层 |
| 3 | M10-03 | M10 | P0 | Page Cache，MAP_SHARED 与 ext4 都依赖 |
| 4 | M10-04 | M10 | P0 | ext4 超级块/inode |
| 5 | M10-05 | M10 | P0 | ext4 目录/extent |
| 6 | M10-08 | M10 | P0 | MAP_SHARED（M11 动态链接前置） |
| 7 | M11-01 | M11 | P0 | ELF 动态段解析 |
| 8 | M11-02 | M11 | P0 | 辅助向量 |
| 9 | M11-03 | M11 | P0 | ld.so 加载 |
| 10 | M11-04 | M11 | P0 | futex WAIT/WAKE |
| 11 | M11-06 | M11 | P0 | TLS FS base |
| 12 | M11-07 | M11 | P0 | 上下文切换恢复 FS |
| 13 | M11-08 | M11 | P0 | clone CLONE_SETTLS |
| 14 | M13-06 | M13 | P0 | sigaction SA_SIGINFO |
| 15 | M13-07 | M13 | P0 | sigaltstack |
| 16 | M13-10 | M13 | P0 | SIGSEGV 投递路径 |
| 17 | M13-01 | M13 | P0 | /proc/self/maps |
| 18 | M14-01 | M14 | P0 | veth |
| 19 | M14-02 | M14 | P0 | bridge |
| 20 | M14-04 | M14 | P0 | OCI spec 解析 |
| 21 | M14-08 | M14 | P0 | 最小记录锁（SQLite 前置） |
| 22 | M14-11 | M14 | P0 | OTA 升级 + 回滚（嵌入式第一价值） |
| 23 | M14-13 | M14 | P0 | 端到端验收（redis + sqlite + OTA + busybox） |

并行支线（仅依赖 M9，建议在主线 1–5 号任务期间启动）：M12-01 → M12-03/04 → M12-07/08 → M12-10/11。

P0 之后按里程碑内部顺序跟进 P1；P2 任务（ioctl 扩展、/proc 深扩展、FHS、TLS 库）放在各里程碑末段或 M14 验收后补。

---

## 8. 工作量汇总与工期估算

| 里程碑 | P0 工作量 | P1 工作量 | P2+P3 工作量 | 小计 |
|---|---|---|---|---|
| M10 | 28 | 21 | 0 | 49 |
| M11 | 20 | 34 | 0 | 54 |
| M12 | 15 | 6 | 6 | 27 |
| M13 | 14 | 13 | 8 | 35 |
| M14 | 32 | 37 | 57 | 126 |
| **合计** | **109** | **111** | **71** | **291 人日** |

工期推演（假设团队 1–2 人，全职投入）：

| 配置 | 关键路径 | 说明 |
|---|---|---|
| 1 人 | 约 14.5–19 个月 | 串行全部任务 + 20% 缓冲 |
| 2 人（主线 + M12 并行） | 约 9–12 个月 | 关键路径 291 − M12(27) ≈ 264 人日 ÷ 2 + M12 并行消化 + 20% 缓冲 |

估算前提：M9 已稳定，`--features full` 编译链已通，QEMU 集成测试框架可复用。若 ext4 写入一致性（M10-06）或 Redis 网络兼容调试（M14-10）超出预期，工期向区间上沿偏移。

---

## 9. 关键风险与前置缓解

| 风险 | 所在 | 影响 | 缓解措施 |
|---|---|---|---|
| ext4 写入一致性难验证 | M10-06 | 数据损坏类 bug 潜伏 | data=journal 完整模式 + QEMU 断电模拟（随机 kill）回归 |
| JVM 对内核兼容性要求最高，调试链长 | M14-15 | 单任务 12 人日可能超支 | 保持 P3 可选；先做 musl 构建可行性评估，不通过则不投入 |
| seccomp profile 误杀业务程序 | M12-12 | Docker 容器内程序异常退出 | 先跑 Docker 默认 profile 全命令集回归，再开放自定义 profile |
| 动态链接定位问题难以区分内核/用户态 | M11-03 | 调试成本高 | 准备 host 上等价环境的对照测试；musl 侧问题先排除再查内核 |
| 40MB 预算超标 | M14-14 | full 模式无法达标 | 每个里程碑末做基线测量（M10-11/M11-11/M12-13/M13-14），超标即排查，不拖到 M14 |
| veth/bridge 与 net namespace 组合复杂度 | M14-01/02 | 容器网络不稳 | 先在 host 网络简单拓扑下验证 veth，再叠加 net ns，最后接 bridge |

---

## 10. 里程碑验收清单（汇总）

| 里程碑 | 核心验收 | 内存基线 |
|---|---|---|
| M10 | ext4 读写 + 重启持久化；MAP_SHARED 共享物理页 | ≤36MB |
| M11 | 动态链接 hello world；pthread 100 线程无泄漏；宿主机交叉编译 Go/Rust/C++ 冒烟；novos-check 通过 | ≤38MB |
| M12 | `docker exec` PTY 交互；seccomp 拦截 `reboot` | ≤39MB |
| M13 | 动态 busybox（musl）启动 + SIGSEGV 捕获 + `/proc/self/maps` 正确 | ≤39MB |
| M14 | `novos run redis` + SQLite 持久化 + OTA 升级回滚 + `novos run busybox` | ≤40MB |
