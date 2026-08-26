//! M2 切片2：同步原语——Spinlock（内核短临界区自旋锁）+ 阻塞 Mutex（内置优先级继承 PIP）。
//!
//! 勘误 §4.2（PIP × 锁层级边界）：PIP 仅提升持锁者的**有效优先级**以解决优先级反转，
//! **不改变全局锁层级顺序**（锁层级由各锁的 acquire 顺序约定 + PhantomData 编译期编码，
//! 见 DESIGN §4.2）；解锁时恢复基础优先级。
//!
//! 勘误 §11：RT 路径强制 Spinlock（关中断 + 自旋，无阻塞），普通任务用阻塞 Mutex（睡眠让出）。
//! M2 切片实现前者（供内核内部使用）与后者（任务间互斥，含 PIP 演示）。

use crate::task;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ---- Spinlock（内核内部，关中断短临界区）----

/// 自旋锁：进入临界区前关中断，防止持锁时被 tick 抢占造成其他核/中断路径自旋等待。
/// 单核下主要价值是**关中断**（临界区原子性）；M2 供内核内部结构用。
pub struct Spinlock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: 单核 + 关中断访问，Send 数据即安全。
unsafe impl<T: Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    pub const fn new(data: T) -> Self {
        Spinlock {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// 加锁：关中断 + 自旋。
    pub fn lock(&self) -> SpinGuard<'_, T> {
        // SAFETY: cli/st 为特权指令。
        unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinGuard { lock: self }
    }
}

/// Spinlock 守卫：Drop 时解锁并恢复中断。
pub struct SpinGuard<'a, T> {
    lock: &'a Spinlock<T>,
}

impl<'a, T> core::ops::Deref for SpinGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: 持锁独占访问。
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for SpinGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 持锁独占访问。
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for SpinGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        // SAFETY: sti 恢复中断。
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
}

// ---- 阻塞 Mutex（PIP）----

/// 等待者上限（固定数组，防堆分配）。
const MAX_WAITERS: usize = 8;

/// 任务互斥锁：未持有时任务睡眠让出 CPU；等待者到来时提升持锁者优先级（PIP）。
///
/// 锁所有权转移语义：`unlock` 直接把所有权交给最高优先级等待者并唤醒它；
/// 被唤醒任务检查 `holder == me` 即获得锁（无需再 CAS，避免竞争窗口）。
pub struct Mutex {
    /// 持有者任务 id（0 = 空闲；idle 任务 id 亦为 0，不会持锁）。
    holder: AtomicUsize,
    /// 等待者队列（固定数组，cli 保护下内部可变）。
    waiters: UnsafeCell<[usize; MAX_WAITERS]>,
    waiter_count: UnsafeCell<usize>,
}

// SAFETY: 单核访问，关键区关中断保护。
unsafe impl Sync for Mutex {}

impl Mutex {
    pub const fn new() -> Self {
        Mutex {
            holder: AtomicUsize::new(0),
            waiters: UnsafeCell::new([0; MAX_WAITERS]),
            waiter_count: UnsafeCell::new(0),
        }
    }

    /// 加锁：失败则登记等待 + PIP 提升持锁者 + 阻塞让出。
    pub fn lock(&self) {
        let me = task::current_id();
        if me == 0 {
            panic!("sync: idle cannot lock");
        }
        loop {
            cli();
            let h = self.holder.load(Ordering::Relaxed);
            if h == 0 {
                self.holder.store(me, Ordering::Relaxed);
                sti();
                return;
            }
            if h == me {
                sti();
                panic!("sync: mutex recursive lock by task {me}");
            }
            if !self.is_waiter(me) {
                if unsafe { *self.waiter_count.get() } >= MAX_WAITERS {
                    sti();
                    panic!("sync: mutex waiter table full");
                }
                self.add_waiter(me);
                // PIP：把持锁者提升到我的优先级（只升不降）
                task::boost(h, task::priority(me));
            }
            sti();
            // 阻塞让出；被唤醒后检查是否已拿到锁（所有权转移）
            task::block_current();
            if self.holder.load(Ordering::Relaxed) == me {
                return;
            }
        }
    }

    /// 解锁：把所有权交给最高优先级等待者（唤醒之），否则置空闲。
    pub fn unlock(&self) {
        let me = task::current_id();
        cli();
        let h = self.holder.load(Ordering::Relaxed);
        if h != me {
            sti();
            panic!("sync: unlock by non-holder (task {me}, holder {h})");
        }
        // PIP 解除：持锁者恢复基础优先级
        task::restore_prio(me);
        if unsafe { *self.waiter_count.get() } > 0 {
            let w = self.pop_highest_waiter();
            self.holder.store(w, Ordering::Relaxed);
            task::wake(w);
        } else {
            self.holder.store(0, Ordering::Relaxed);
        }
        sti();
    }

    fn is_waiter(&self, id: usize) -> bool {
        // SAFETY: cli 保护下访问内部可变字段。
        let w = unsafe { &*self.waiters.get() };
        let c = unsafe { *self.waiter_count.get() };
        w[..c].contains(&id)
    }

    fn add_waiter(&self, id: usize) {
        // SAFETY: cli 保护下访问内部可变字段。
        let w = unsafe { &mut *self.waiters.get() };
        let c = unsafe { &mut *self.waiter_count.get() };
        w[*c] = id;
        *c += 1;
    }

    /// 取优先级最高的等待者（移除并返回）。
    fn pop_highest_waiter(&self) -> usize {
        // SAFETY: cli 保护下访问内部可变字段。
        let w = unsafe { &mut *self.waiters.get() };
        let c = unsafe { &mut *self.waiter_count.get() };
        let mut best_idx = 0;
        let mut best_prio = u8::MIN;
        for i in 0..*c {
            let p = task::priority(w[i]);
            if p > best_prio {
                best_prio = p;
                best_idx = i;
            }
        }
        let r = w[best_idx];
        w.copy_within(best_idx + 1..*c, best_idx);
        *c -= 1;
        r
    }
}

/// 关中断（任务上下文临界区保护；与 `sti` 配对）。
pub fn cli() {
    // SAFETY: cli 为特权指令。
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

/// 开中断。
pub fn sti() {
    // SAFETY: sti 为特权指令。
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
}
