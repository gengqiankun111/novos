# 山水观心操作系统 单元测试覆盖说明（TEST-COVERAGE）

> 本文档说明内核当前单元测试的**覆盖范围与分层**：每个用例验证什么、覆盖哪些代码路径、
> 如何运行。随测试补充持续更新。
> 对应代码：`kernel/src/fs.rs`（`status_tests` 模块）、`kernel/src/rbtree.rs`（`tests` 模块）。
> 运行环境：host `cargo test --lib` 已打通（需 VS Build Tools C++ 工作负载提供 MSVC link.exe）。

---

## 1. 测试分层

| 层 | 载体 | 运行方式 | 验证内容 | 状态 |
|---|---|---|---|---|
| **L1 host 单元测试** | `#[cfg(test)]` 模块 | `cargo test -p shanshui-guanxin-kernel --lib` | 纯逻辑/纯函数，无内核状态依赖 | ✅ 26/26 |
| **L2 QEMU 启动自测** | 内核内 `status_self_test()` | `make test`（boot 模式） | 真实内核上下文生成 `/proc/self/status`，断言 `fs/proc: status self-test PASS` | ✅ |
| **L3 QEMU 集成测试** | 用户态 shell `statustest`/`maptest` | `make test`（shell 模式） | 经真实 syscall（open/read）读取 `/proc/self/status`、`/proc/self/maps` | ✅ |

> **为何三层**：内核为 `no_std` 裸机，部分行为只能在真实内核中验证（如页表遍历算 RSS、
> 当前任务 pid/name 取值）；纯格式/纯数据结构逻辑则可在 host 快速回归。三层互补。

---

## 2. status_body 覆盖矩阵（`fs::status_tests`，18 个）

> 被测对象：`status_body(name, pid, rss_kb, vsize_kb)`——`/proc/self/status` 的纯格式化函数。

| # | 用例 | 验证点 | 覆盖的代码路径/边界 |
|---|---|---|---|
| 1 | `has_all_fields` | 16 个必需字段齐全 | 全部字段行生成 |
| 2 | `embeds_name_pid` | Name 首行、Tgid/Pid/PPid 内嵌 | 参数 → 占位符替换 |
| 3 | `field_order_is_linux_like` | Name<Uid<VmRSS<Threads 相对顺序 | 行序（Linux 语义） |
| 4 | `rss_and_vsize_values_are_numeric` | VmRSS/VmSize 数值正确 | 数值透传 |
| 5 | `vmpeak_tracks_rss` | VmPeak==VmRSS 数值 | 当前实现口径（无历史峰值） |
| 6 | `threads_is_one` | Threads 固定 1 | 进程级视图语义 |
| 7 | `uid_gid_are_root` | Uid/Gid 全 0 | 单用户 root 语义 |
| 8 | `every_line_has_field_colon` | 每行 `字段:` 结构 | 整体结构健全性 |
| 9 | `empty_name_and_zero_pid` | 空任务名、pid=0 | 空串/最小值边界 |
| 10 | `max_pid_boundary` | pid=`u32::MAX` | 最大值边界 |
| 11 | `zero_metrics_right_aligned_width6` | 零值 `{:>6}` → `"     0 kB"` | 右对齐格式 |
| 12 | `large_metrics_no_truncation` | 4 GiB（4_194_304 kB）不截断 | 超宽数值格式 |
| 13 | `ends_with_newline_and_no_blank_lines` | 尾换行、无空行 | 行终止符/空白 |
| 14 | `fields_are_tab_separated` | `Name:\t`、`Uid:\t0\t0\t0\t0` | Tab 分隔 |
| 15 | `state_and_static_lines` | State/Cpus_allowed/Seccomp/NoNewPrivs | 静态字段 |
| 16 | `pid_tgid_match_and_ppid_zero` | Pid==Tgid、PPid=0 | 身份字段一致性 |
| 17 | `metric_lines_end_in_kb` | 所有 Vm* 行 ` kB` 结尾 | 单位一致性 |
| 18 | `no_carriage_returns` | 无 CR | Linux 行风格 |

**未覆盖/已知口径**（设计如此，非缺陷）：
- `VmPeak` 以当前 RSS 计（无历史峰值跟踪，后续引入峰值计数器后需更新用例 5）；
- `Uid/Gid` 固定 root（单用户模型，引入多用户/凭据后需更新用例 7）；
- `Threads` 固定 1（无 tgid 跟踪，引入线程模型后需更新用例 6）。

---

## 3. rbtree 覆盖矩阵（`tests` 模块，8 个）

> 被测对象：固定池红黑树 `RbTree`（`insert(id,key)`/`remove(id)`/`min()`/`is_empty`，
> 供 CFS runqueue 使用，节点池 16 槽）。

| # | 用例 | 验证点 | 覆盖的代码路径/边界 |
|---|---|---|---|
| 1 | `rbt_insert_remove_random` | 8 节点乱序插入、中序有序、黑高一致、删除 | insert/insert_fixup/remove/delete_fixup 主干 |
| 2 | `empty_tree_queries` | 空树 is_empty/min=None/count=0 | 边界（根=SENTINEL） |
| 3 | `single_node_roundtrip` | 单节点插删 | 根即最小节点 |
| 4 | `duplicate_keys_both_retained` | 等键（key=5,5）右分支链，min 恒为最先插入者 | 等键比较（`key <` 严格小于） |
| 5 | `extreme_keys_min` | key=`u64::MIN/MAX/MAX/2`，删最小后 min 正确 | 极端 key + 删除后黑高 |
| 6 | `full_pool_roundtrip` | 填满 16 槽（`MAX_RB_NODES`）再全删 | 池容量上限 + 逐槽清空 |
| 7 | `remove_min_repeatedly_ascends` | 反复取 min+删，min 严格升序 | 删除后最左指针维护 |
| 8 | `min_untouched_by_removing_largest` | 删最大节点不影响 min | 非最小节点删除路径 |

**覆盖到的内部函数**：`insert` → `insert_fixup`（含旋转 `rotate_left/right`）、
`min`/`tree_min`、`remove` → `transplant`/`delete_fixup`、`is_empty`、黑高校验（`black_height`）。

**未覆盖**（可扩展方向）：
- `delete_fixup` 的"红兄弟"（case 1）分支——现有用例未显式构造红兄弟场景；
- 最大节点查询（`max()` 不存在，若引入需补用例）；
- 越界 id（`debug_assert!(id < MAX_RB_NODES)` 属调用方契约，host 测试断言 panic 需 `#[should_panic]`）。

---

## 4. L2/L3 运行期覆盖

### 4.1 启动自测 `status_self_test()`（L2）

在真实内核启动时生成 `/proc/self/status` 并断言：
- 字段齐全（Name/State/Tgid/Pid/PPid/Uid/Gid/FDSize/VmPeak/VmSize/VmRSS/Threads/Seccomp）；
- 相对顺序 Name<Uid<VmRSS<Threads；
- `VmRSS`/`VmSize` 数值可解析（`split_whitespace` → `parse::<u64>`）；
- `Threads=1`、`Uid/Gid=0`、`VmPeak==VmRSS`。

覆盖 L1 无法触达的真实路径：`task::current_pid/current_name/current_cr3` →
`page_table::count_user_pages`（4 级页表遍历统计 RSS）→ `vmm::vsize_bytes`。

### 4.2 用户态集成测试 `statustest` / `maptest`（L3）

- `statustest`：经真实 syscall（`open/read/close`，每次 read 上限 512B 的多段累积读取）
  读取 `/proc/self/status`，校验 Name/Uid/VmRSS/Threads 字段命中并打印 `VmRSS` 行；
- `maptest`：`/proc/self/maps` 的映射注册表输出（段数/`/init`/`[stack]`/`r-xp`）；
- 断言位于 `scripts/test-boot.ps1`（boot 模式：`fs/proc: status self-test PASS`；
  shell 模式：`statustest: status ok`、`maptest: maps ok` 等）。

---

## 5. 运行方式

```bash
# L1：host 单元测试（26 个）
cargo test -p shanshui-guanxin-kernel --lib

# L2 + L3：QEMU 集成（boot 模式含启动自测，shell 模式含 statustest/maptest）
make test

# 仅构建真实内核
make build
```

**host 测试前置条件**：MSVC 链接器可用（VS Build Tools C++ 工作负载，link.exe 于
`C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\<ver>\bin\Hostx64\x64\`）。
内核侧为支持 host 测试所做的适配见 [DEVELOPMENT.md](DEVELOPMENT.md) 测试章节与
`kernel/src/lib.rs` 的 `#[cfg(not(test))]` 说明（boot.asm/global_allocator/extern 桩）。

---

## 6. 覆盖统计与演进

| 被测对象 | 用例数 | 覆盖重点 |
|---|---|---|
| `status_body`（fs.rs） | 18 | 字段/顺序/格式/数值边界 |
| `RbTree`（rbtree.rs） | 8 | 插删/等键/极值/满池/删除后 min |
| 启动自测（L2） | 1 项复合断言 | 真实内核 `/proc/self/status` 管线 |
| 集成测试（L3） | 2 命令 | syscall 真实读路径 |

> 新测试加入时：L1 放对应模块 `#[cfg(test)]`；涉及真实内核状态的断言走 L2/L3
> （并同步 `scripts/test-boot.ps1` 的 needle）。本文件随 `cargo test` 计数同步更新。
