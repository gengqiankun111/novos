# Novos-OS 设计勘误与补救方案（v0.1-errata）

> 基于 2026-08-26 深度架构评审（SMP/内存/网络/调度/安全 12 项），列出 DESIGN.md 中未覆盖或过于乐观的工程问题，并给出最小化实施方案。
> 优先级分三档：**① 必须立即改**（否则产品变砖）→ **② 必须评估期改**（否则性能极差）→ **③ 架构预留**（不改代码，改注释/接口）。

---

## 一、① 必须立即改（否则产品变砖）

### 1. OverlayFS 写放大导致 OOM（原 DESIGN §4.4/§3.7）

**问题**：容器内写日志（如 `/var/log`）触发 OverlayFS 全量 copy-up。lower 文件 10MB 只改 1 字节，第一次写入会把整个 10MB 复制到 upper；32MB 内存下并发写多个大文件瞬间挤爆 Page Cache。

**方案**：
- **稀疏 copy-up（extent-based）**：只复制被修改的文件块，而非整个文件（upper 以 sparse file 呈现，未修改区域仍指向 lower）；
- **日志强制 volatile/tmpfs**：容器日志目录（`/var/log`、systemd journal 等价物）默认挂载 tmpfs，禁止持久化日志触发 copy-up；
- 只读层与写层通过 `SparseLower` 追踪每个 extent 是否已 copy-up。

### 2. Futex 物理页哈希在 COW 下的致命缺陷（原 DESIGN §13.7）

**问题**：Futex 按物理页地址哈希。fork() 后父子共享物理页（COW），写入触发分裂后物理页地址变化：新页等待队列为空、旧页等待队列无人唤醒 → 同步锁永久睡眠。

**方案**：
- Futex Key 改为**逻辑组合**：(Inode, 文件偏移) 用于文件映射；**匿名虚拟区 + 虚拟地址**用于匿名共享内存（与 Linux `get_futex_key` 对齐）；
- **COW 迁移**：物理页分裂时，把旧页等待队列迁移到新物理页（`page->futex_queue` 随页迁移）；
- 实现顺序：M2 先按逻辑键哈希；M4 加 COW 迁移（依赖 VFS inode 结构就位）。

### 3. PID 1（Init）崩溃无自愈机制（原 DESIGN §1.2）

**问题**：Init 进程段错误崩溃后无备选 init，系统直接 panic/空转——无人值守工业网关等于变砖。

**方案**：
- **内核级热备 init**：检测到 PID 1 退出时，不立即 panic，尝试执行 `rescue_init`——静态编译在内核 `.rodata` 的最小 Shell（busybox 子集，随内核镜像打包）；
- rescue 也失败 → 触发**硬件 Watchdog 复位**（喂狗线程与 PID 1 监控解耦）；
- 内核维护 `init_pid` + `init_death` 回调链：`panic → rescue → watchdog reset` 三级兜底。

---

## 二、② 必须评估期改（否则性能极差）

### 4. 零拷贝网络栈重构（替代 DESIGN §3.8 Skb 设计）

**问题**：`struct Skb { data: Vec<u8> }` 每次收发 3 次拷贝（DMA→内核 Vec→socket 缓冲→重传队列）+ 频繁分配；32MB 内存下 100Mbps 即占满 CPU。

**方案**：
- **内存池 + 引用计数**：`Skb { ptr: *mut u8, len }`，ptr 指向预分配 DMA 池中的页；
- 接收：驱动直接拿池中页给协议栈；clone 只 `Arc` 增引用；协议层处理完归还池；
- 发送路径零拷贝：`sendto` 引用计数组装 skb，TCP 重传队列复用同一页；
- 池大小按 §5.3 预算设上限（如 256KB），超限走内核堆回落路径。

### 5. 定时器最小堆在大量 TCP 连接下 O(n) 过高（原 DESIGN §4.5）

**问题**：每个 TCP 连接（重传/保活）+ Conntrack 条目入最小堆，1000 连接 + 1000Hz tick 时堆维护开销巨大。

**方案**：
- **分层时间轮（Hierarchical Timing Wheels）**替代最小堆：O(1) 入队、O(1) 滴答推进、无需排序；
- 五层轮（1ms/16ms/256ms/4s/64s 桶）覆盖到分钟级保活；
- 最小堆保留给"少量高精度"场景（如内核自身延迟记账），大量连接走时间轮。

### 6. 文件系统缺少 I/O 调度合并（原 DESIGN §13.3）

**问题**：Block I/O 层简单 FIFO，容器日志 4K 随机写与 OCI 拉取 128K 顺序读混排 → eMMC/SD 寻道延迟导致应用超时。

**方案**：
- **极简电梯调度（Deadline 简化版）**：READ 严格优先于 WRITE（读通常阻塞用户态进程）；写按相邻 LBA 合并；
- 每个请求带 deadline（读 100ms / 写 500ms 默认），到期提升优先级防饿死；
- 代码量约 300 行（Linux blk-mq ~2 万行的 1.5%）。

---

## 三、③ 架构预留（不改代码，改注释/接口）

### 7. SMP 预热架构（替代 DESIGN §11 的"评估期"）

**问题**：现代 ARM Cortex-A（A53/A72）几乎全多核；"M9 评估"会把 SMP 拖成后期推倒重来。单运行队列 + 单 Buddy + 无 per-CPU 结构在双核上 Spinlock 竞争严重（吞吐量可能仅单核 1.3 倍）。

**方案**：
- **第一版就引入 per-CPU 数据结构占位符**（哪怕初始只初始化 CPU 0）：`per_cpu!` 宏 + `cpu_rq(cpu_id)` 接口；
- 调度器红黑树预留 `cpu_rq(cpu_id)` 访问器，SMP 打开时只是"多实例化"而非重构；
- Buddy/Slab/页表提前标注 per-CPU 属性，单核 = 特例（nr_cpu=1）。

### 8. PlatformInfo 启动信息抽象（原 DESIGN §1.3/§17）

**问题**：x86 依赖 Multiboot、ARM 依赖 Device Tree（FDT），两种启动信息数据结构完全不同；当前 boot.asm 直接解析 Multiboot 无法移植。

**方案**：
- arch 层统一抽象 **`PlatformInfo`**：内存布局、中断控制器基址、PCIe MMIO、时钟频率；
- x86：`boot.rs` 解析 Multiboot/PVH 后填充 PlatformInfo；ARM：`boot.S` 后调 `dtb_parse()` 填充；
- **内核核心（内存管理）只认 PlatformInfo，不认 Multiboot 或 DTB**。

### 9. Slab 分配器内存泄漏隐患（原 DESIGN §3.1）

**问题**：`SlabCache` 用 `Vec<*mut u8>` 存空闲对象，内存压力下 Vec 扩容触发递归分配（想释放内存却先要分配）；Vec 本身内存不归还 Buddy。

**方案**：
- 参照 Linux `kmem_cache`，**禁止 Slab 管理结构用动态数组**；
- 改用**侵入式空闲链表**（`free_list: *mut u8` 单向链表）：对象释放时头插，零额外内存；
- 每个 slab 页内的空闲对象用 `next` 指针串起来（对象头 8 字节）。

---

## 四、深度问题（补充入 DESIGN 对应章节）

### 10. 物理内存碎片化（原 DESIGN §4.1 末尾）

**问题**：32MB 长时间运行后频繁 4K 分配释放导致碎片化；容器启动需连续 2MB 大页或 DMA 时 order 9 分配必然失败（即使总空闲 10MB）。

**方案**：
- **可移动页（MIGRATE_MOVABLE）**：用户态匿名页 + Page Cache 标记 MOVABLE；内核关键结构（页表等）UNMOVABLE；
- **compact_zone()**：order ≥ 3 分配失败时触发，把低阶可移动页拷贝合并成高阶连续区；
- 收益：大页/DMA 分配成功率显著提升，碎片不蔓延到内核结构。

### 11. 锁序死锁（优先级继承）实现未闭环（原 DESIGN §4.2）

**问题**：Mutex 睡眠导致调度；低优先级任务持锁时高优先级任务在 Mutex::lock 睡眠并触发抢占，低优先级可能拿不到 CPU 释放锁；且"提升持锁者优先级"本身要获取持锁者调度锁，多核下引入新死锁。

**方案**：
- **M2 强制**：RT 任务禁止使用睡眠锁，只用 Spinlock（关抢占）；
- 普通 CFS 任务用 Mutex 时，**关闭内核抢占（preempt_disable()）直到锁释放**——用空间换确定性；
- PIP 提升路径的调度锁获取用 trylock + 无锁读，避免提升自身成为死锁源。

### 12. Seccomp 仅过滤系统调用号毫无意义（原 DESIGN §13.5）

**问题**：Docker 默认 Seccomp 主要拦截危险**参数**（如 mount 挂载 proc 的行为）。仅过滤 nr，攻击者调合法 `openat` 打开 `/etc/shadow` 即可逃逸。

**方案**：
- BPF 解释器升级支持**参数值匹配**（eq/ne/masked_eq）；
- 至少覆盖 10 个高风险调用参数校验：`mount`（源/目标路径）、`ptrace`、`openat`、`execve`、`reboot`、`clone`（flags）等；
- 先实现最小参数校验（用户态策略表驱动），完整 BPF ISA 解释器后置。

---

## 行动优先级总表

| # | 项 | 优先级 | 落地里程碑 | 修改对象 |
|---|---|---|---|---|
| 1 | OverlayFS 稀疏 copy-up + 日志 tmpfs | ① 立即 | M8 | DESIGN §4.4、DEVELOPMENT M8 |
| 2 | Futex 逻辑键 + COW 迁移 | ① 立即 | M2（键）/M4（迁移） | DESIGN §13.7、DEVELOPMENT M2/M4 |
| 3 | PID 1 崩溃自愈（rescue_init + watchdog） | ① 立即 | M3 | DESIGN §1.2、DEVELOPMENT M3 |
| 4 | 零拷贝 Skb 内存池 | ② 评估期 | M5 | DESIGN §3.8、DEVELOPMENT M5 |
| 5 | 分层时间轮 | ② 评估期 | M9（或 M5 网络热路径） | DESIGN §4.5、DEVELOPMENT M9 |
| 6 | I/O 电梯调度 | ② 评估期 | M13 | DESIGN §13.3、DEVELOPMENT M13 |
| 7 | SMP per-CPU 预热 | ③ 预留 | M2（占位）/M9（打开） | DESIGN §11、DEVELOPMENT M2 |
| 8 | PlatformInfo 抽象 | ③ 预留 | M0 起（逐步） | DESIGN §1.3/§17、kernel/boot |
| 9 | Slab 侵入式空闲链表 | ③ 预留 | M1 | DESIGN §3.1、DEVELOPMENT M1 |
| 10 | 内存碎片化 compact + 可移动页 | 补充 | M1（标记）/M9（compact） | DESIGN §4.1 |
| 11 | 锁序闭环（RT 自旋锁 + 关抢占） | 补充 | M2 | DESIGN §4.2 |
| 12 | Seccomp 参数过滤 | 补充 | M12 | DESIGN §13.5、DEVELOPMENT M12 |
