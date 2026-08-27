//! SMP 预热占位（勘误③ §11）：`per_cpu!` 宏 + `cpu_rq(cpu_id)` 访问器。
//!
//! 第一版单核（`NR_CPUS = 1`），但**调度器从第一天按 `cpu_rq(cpu_id)` 组织**
//! （CFS 就绪红黑树挂载在每 CPU runqueue 上）；SMP 打开 = 数组扩容 +
//! `this_cpu()` 改为按 APIC id 索引，而非重构调度器。详见 DESIGN.md §11.1。

#![allow(static_mut_refs)]

use crate::rbtree::RbTree;

/// 支持的 CPU 数（第一版单核）。
pub const NR_CPUS: usize = 1;

/// 每 CPU runqueue：CFS 就绪红黑树（按 vruntime）。
pub struct Rq {
    pub rbt: RbTree,
    /// 占位：SMP 时每 CPU 一把自旋锁（锁序见 DESIGN §4.2）。
    pub rq_lock: u8,
}

impl Rq {
    pub const fn new() -> Self {
        Rq { rbt: RbTree::new(), rq_lock: 0 }
    }
}

/// 每 CPU runqueue 表（`cpu_rq(cpu_id)` 访问）。
static mut RUNQUEUE: [Rq; NR_CPUS] = [Rq::new(); NR_CPUS];

/// 获取指定 CPU 的 runqueue（可变，供调度器在关中断区使用）。
///
/// # Safety
/// 单核：仅 CPU0 有效；多核时须持有该 CPU 的 rq 锁。
pub unsafe fn cpu_rq(cpu: usize) -> &'static mut Rq {
    &mut RUNQUEUE[cpu.min(NR_CPUS - 1)]
}

/// 当前 CPU id（单核恒 0；SMP 时读 APIC id）。
pub fn this_cpu() -> usize {
    0
}

/// `per_cpu!` 宏：声明 per-CPU 变量。
///
/// 单核展开为长度为 1 的数组；SMP 时数组扩到 `NR_CPUS`，
/// 经 `this_cpu_var!` 按下标访问（`&$name[this_cpu()]`）。
///
/// 示例：
/// ```rust
/// per_cpu!(IRQ_COUNT: u64 = 0);
/// // 访问：`&IRQ_COUNT[this_cpu()]`
/// ```
#[macro_export]
macro_rules! per_cpu {
    ($(#[$attr:meta])* $name:ident: $t:ty = $init:expr) => {
        $(#[$attr])*
        static mut $name: [$t; $crate::smp::NR_CPUS] = [$init; $crate::smp::NR_CPUS];
    };
}
