# 山水观心操作系统：问题排查与修改记录

> 记录开发过程中遇到的问题、根因分析、修复方案与验证结果，便于回溯与避免重蹈覆辙。
> 按时间倒序排列；每条记录含：现象 → 根因 → 修复 → 验证。

---

## 2026-08-28 批次

### P9. fcntltest 子进程 F_SETLKW 阻塞后父进程得不到调度（挂死）

- **现象**：M14 最小记录锁测试中，子进程 `F_SETLKW` 阻塞等待父进程解锁，但父进程永远不运行，测试挂死。
- **根因**：`task::sleep_ticks(n)` 的实现是**单次 `hlt`**（非循环）：置 `Sleeping` 并出队后 `hlt` 一次即返回。下个 tick 的唤醒循环把子进程重新入队并选中它，**rq 永不为空**，父进程（task 0 不在 rq 中，仅在 rq 为空时才被选中）永远得不到调度，锁永远不会被释放。
- **修复**：`flock::set_lock` 阻塞路径改用 `sleep_deadline(ticks + 2)`——真实睡眠窗口内 rq 为空，父进程被调度并完成解锁，子进程醒来后重试成功。
- **验证**：`fcntltest` 全链路通过（父锁→子冲突 EAGAIN→F_GETLK 查得父锁→父解锁→子 F_SETLKW 获锁）。
- **经验**：本内核 `sleep_ticks` 是"让出一个 tick"的轻量语义，不适合作为多进程互斥的等待原语；跨进程等待必须用 `sleep_deadline`（真实睡眠窗口）。

### P8. 用户二进制增大后 sigtest 恢复点携带陈旧 callee-saved 寄存器（内核崩溃）

- **现象**：用户态二进制（帮助文本/新命令）增长到阈值后，shell 测试在 sigtest 之后崩溃：内核 `sys_write` 收到损坏的 `&str` 指针（`0xffffffffffffb6fc` 等），memchr 扫越界地址触发内核 #PF。且与内核/用户二进制布局强相关（缩小二进制即消失）。
- **根因**：sigtest 的 `jmp_set` 只保存 `rsp/rbp/rip`，`rt_sigreturn` 恢复后，跨信号窗口的 **callee-saved 寄存器（rbx/r12-r15）带回的是 #PF 时刻的旧值**，而非 `jmp_set` 保存点时刻的值。编译器在新二进制里依赖这些寄存器跨窗口保持 → 状态错乱 → 后续 `print` 的 `&str` 胖指针被破坏。
- **修复**：重构 sigtest 恢复点为**无参函数重入**（`sigtest_resume()`）：恢复点后只做一次干净的函数调用，进入函数即建立全新栈帧并重新初始化全部寄存器状态；参数（备用栈基址）经静态 `SIGTEST_ALT_BASE` 传递。
- **验证**：任意增大用户二进制（新增命令/帮助文本）不再崩溃，三层测试全绿。
- **经验**：跨信号/异常窗口的恢复必须保证被恢复点依赖的寄存器状态自洽；最稳做法是"恢复点 = 干净函数调用"而不是"恢复点 = 内联代码"。

### P7. M13-09 sigreturn 增强与布局敏感崩溃交互（一度回退）

- **现象**：M13-09（deliver 阻塞本信号 + rt_sigreturn 恢复 mask）首次实现后触发 P8 的崩溃；与 P8 修复前无法共存。
- **处理**：先修复 P8（sigtest 恢复点重构），再重新应用 M13-09 内核改动，即通过。**顺序依赖**：P8 是前置 bug。
- **验证**：`sigreent` 通过（handler 期间 mask=512，返回后恢复 0）。

### P6. PowerShell 脚本 `@(...)` 数组尾逗号 + 注释导致解析错误

- **现象**：test-boot.ps1 在 needle 数组末尾加元素后报 `Missing expression after ','` / `Missing closing ')'`。
- **根因**：数组最后元素带逗号后紧跟注释行再 `)`，PowerShell 解析器视为"逗号后缺表达式"；多次编辑的 CRLF/LF 混合也可能干扰。
- **修复**：用 `[System.Management.Automation.Language.Parser]::ParseFile` 定位；保证最后一个元素无尾逗号、注释移到 `)` 之后。
- **经验**：编辑 PowerShell 数组时保持"最后元素无尾逗号"；用 Parser API 快速校验语法。

### P5. shell 测试采集窗口不足，尾部命令输出丢失

- **现象**：新增命令放在 `$cmd` 末尾（sigtest 之后）时，其输出未被 drain 窗口捕获 → 断言 MISS。
- **修复**：把需要断言的命令移到命令列表中靠前位置（如 `jvmsmoke` 移到 sigtest 之前）。
- **经验**：`$cmd` 尾部命令可能来不及执行/输出；新测试命令应插入列表中段。

### P4. M13 timerfd 测试依赖真实时钟导致采集窗口超时

- **现象**：timerfd 设 300ms + epoll 阻塞 1s，超出 shell 测试输出采集窗口 → FAIL。
- **修复**：测试定时器缩短到 20ms（2 tick）、epoll 超时 200ms；并给 `epoll_wait` 增加 timeout 阻塞语义（`sleep_deadline`）。
- **附带修复**：`sleep_deadline` 补 task 0 保护（rq 唤醒循环不覆盖 task 0，task 0 改为 hlt 自旋等 tick，volatile 读 TICKS 防编译器提升）。

### P3. epoll_wait 阻塞后 task 0 永不唤醒（自旋死循环）

- **现象**：`sleep_deadline` 对 task 0 无保护，`on_timer_tick` 唤醒循环 `for i in 1..MAX_TASKS` 不覆盖 task 0 → 永久挂起。
- **修复**：`sleep_deadline` 加 `cur == 0` 分支：hlt 自旋等 tick（volatile 读 TICKS，避免 `options(nomem)` 导致负载被提升）。

### P2. rbtree 测试缺 `alloc::vec` 导入 / host 测试链接失败

- **现象**：`cargo test --lib` 报 `E0425/E0433 cannot find Vec`；boot.asm `.note.Xen` 在 host COFF 汇编器报错；4 个裸机符号未解析。
- **修复**：补 `use alloc::vec; use alloc::vec::Vec;`；`.note.Xen` 拆到 boot_note.asm 并 `#[cfg(not(test))]`；`#[cfg(test)]` 桩补齐 gdt64/tss 等符号；`#[global_allocator]` 仅非 test。

### P1. 信号帧偏移错误（siginfo 位置）

- **现象**：handler 读 siginfo 得到错误数据。
- **根因**：ExceptionFrame 实际 22×u64=176B，siginfo 偏移 +0xE8 而非 +0xC0。
- **修复**：SigFrame ABI 按实际偏移对齐（userspace SavedRegs 与内核 ExceptionFrame 严格一致）。

---

## 更早历史（M0–M12，据 git 提交记录回溯）

### P10. UART FIFO 16 字节溢出导致 shell 丢命令（M11）

- **现象**：test-boot.ps1 一次性注入全部命令时，guest UART FIFO（16 字节）溢出，部分命令丢失或 shell 等换行卡死。
- **修复**（提交 d1c9c91）：脚本改为**逐条注入命令 + 间隔**（`foreach + Start-Sleep`），保证不丢命令。
- **经验**：串口注入必须考虑 FIFO 深度；大批量输入需限速。

### P11. host `cargo test --lib` 无法编译裸机内核（M11）

- **现象**：boot.asm 的 `.note.Xen`（GNU/ELF 语法）在 host（COFF 目标）汇编器报错；且 gdt64/tss/stub_base/pdpt/syscall_entry 等裸机符号未解析；`#[global_allocator]` 在测试二进制中引发死循环。
- **修复**（提交 3e20a7c / fe913ca / f4bc21b）：`.note.Xen` 拆到 boot_note.asm 并 `#[cfg(not(test))]`；补 `#[cfg(test)]` 符号桩；`#[global_allocator]` 仅非 test；补 `alloc::vec` 导入。
- **验证**：`cargo test --lib` 26 用例全通过。

### P12. 架构评审发现的遗留缺陷（设计层修正，M2–M8 阶段）

- **来源**：提交 7befa60（12 项架构评审勘误）与 d591789（遗留缺陷修正）。
- **记录**：
  - **PID1 自愈反跳计时器**：崩溃自愈需防抖，避免反复重启；
  - **PIP 与锁层级边界**：优先级继承需与锁序编码协调，防止提升路径破坏锁序；
  - **TCP 已确认段批量回收**：避免逐个回收的性能与内存碎片；
  - **OverlayFS 写放大 / Futex COW 等待队列迁移 / 时间轮 / 碎片化 / Seccomp 参数匹配**：勘误落地（DESIGN_ERRATA）。

### P13. Windows 构建工具链适配（M0/M1）

- **现象**：便携 make 的 MSYS bash 丢弃 Windows PATH 条目，找不到 cargo；QEMU 不在 PATH。
- **修复**（提交 322e938 / 09fc346）：Makefile 用 `cygpath` 补 cargo bin 路径；自动探测 QEMU 完整路径；llvm-objcopy 按宿主选择。

### P14. 误提交清理（M0）

- **现象**：`loaders.cache` 被误提交。
- **修复**（提交 9ae90ad）：`git rm` 移除并加入忽略。

---

## 历史修复速查

| 日期 | 问题 | 修复 |
|---|---|---|
| 2026-08-28 | M13-09 与 P8 交互崩溃 | 先修 P8，再应用 M13-09 |
| 2026-08-28 | mtabtest root 断言 needle 错 | `/ / tmpfs` → `/dev/root / tmpfs` |
| 2026-08-28 | sigmasktest 挂死（jmp_set 长跳与主栈信号帧交互） | 专用不重定向 handler `segv_handler_plain` |
| 2026-08-28 | `syscall3(SYS_EPOLL_CREATE, 1, 0)` 参数不足 | 改 3 参数 |
| 2026-08-28 | `syscall5(SYS_EPOLL_WAIT, ..., 1, 0)` 缺参 | 补第 5 参 |
| 2026-08-28 | fs.rs `comp.as_str()` 触发 unstable `str_as_str` | 改 `*comp` |
