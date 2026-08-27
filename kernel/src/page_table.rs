//! M3 切片3/4：用户进程页表 + 进入 ring3。
//!
//! 构建独立用户进程 4 级页表：PML4[0] 复用内核恒等映射（supervisor），
//! 其余项按需分配中间页表并映射用户页（USER 位）。`enter_user` 经 `iretq`
//! 切换到 ring3，供 ELF 加载器（elf.rs）与手写机器码共用。

use crate::gdt::{USER_CODE_SEL, USER_DATA_SEL};
use crate::mm;

/// 页表项标志（位）。
pub const P_PRESENT: u64 = 1 << 0;
pub const P_WRITABLE: u64 = 1 << 1;
pub const P_USER: u64 = 1 << 2;
pub const P_HUGE: u64 = 1 << 7; // 2MB 大页（PS）

/// 用户栈默认虚拟地址（512GB + 32MB）。
pub const USER_STACK_VADDR: u64 = 0x80_0000_0000 + 0x20_00000;

// boot.asm 导出的内核 PDPT（恒等映射 0~1GB）。
#[cfg(not(test))]
extern "C" {
    static pdpt: u8;
}
// host 单测桩：boot.asm 不参与测试链接，提供同名静态占位。
#[cfg(test)]
pub static pdpt: u8 = 0;

/// 用户进程页表。
pub struct UserPageTable {
    pub pml4: usize,
}

impl UserPageTable {
    /// 创建根页表：清零并挂内核恒等映射（PML4[0]，supervisor）。
    pub fn new() -> UserPageTable {
        let pml4 = mm::alloc_pages(0);
        assert!(pml4 != 0, "pt: pml4 alloc failed");
        // SAFETY: 分配的物理页，恒等映射下可直接访问。
        unsafe {
            core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
            let kpdpt = core::ptr::addr_of!(pdpt) as u64;
            *(pml4 as *mut u64) = kpdpt | P_PRESENT | P_WRITABLE;
        }
        UserPageTable { pml4 }
    }

    /// 将物理页映射到用户虚拟地址（4K 粒度，中间表按需分配）。
    pub fn map_page(&mut self, vaddr: u64, phys: usize, flags: u64) {
        let l4 = ((vaddr >> 39) & 0x1FF) as usize;
        let l3 = ((vaddr >> 30) & 0x1FF) as usize;
        let l2 = ((vaddr >> 21) & 0x1FF) as usize;
        let l1 = ((vaddr >> 12) & 0x1FF) as usize;

        let pml4 = self.pml4 as *mut u64;
        // SAFETY: 页表页均为本进程分配，恒等映射下可访问；中间表按需分配。
        unsafe {
            let e3 = pml4.add(l4);
            let l3t = if *e3 & P_PRESENT == 0 {
                let p = alloc_table();
                *e3 = p as u64 | P_PRESENT | P_WRITABLE | P_USER;
                p as *mut u64
            } else {
                (*e3 & !0xFFF) as *mut u64
            };
            let e2 = l3t.add(l3);
            let l2t = if *e2 & P_PRESENT == 0 {
                let p = alloc_table();
                *e2 = p as u64 | P_PRESENT | P_WRITABLE | P_USER;
                p as *mut u64
            } else {
                (*e2 & !0xFFF) as *mut u64
            };
            let e1 = l2t.add(l2);
            let l1t = if *e1 & P_PRESENT == 0 {
                let p = alloc_table();
                *e1 = p as u64 | P_PRESENT | P_WRITABLE | P_USER;
                p as *mut u64
            } else {
                (*e1 & !0xFFF) as *mut u64
            };
            let e0 = l1t.add(l1);
            *e0 = phys as u64 | flags;
        }
    }

    /// 查询虚拟地址对应的物理页帧（4 级表逐级查找；未映射返回 None）。
    /// 供 ELF 加载器合并相邻段共享页用。
    pub fn translate(&self, vaddr: u64) -> Option<usize> {
        let l4 = ((vaddr >> 39) & 0x1FF) as usize;
        let l3 = ((vaddr >> 30) & 0x1FF) as usize;
        let l2 = ((vaddr >> 21) & 0x1FF) as usize;
        let l1 = ((vaddr >> 12) & 0x1FF) as usize;
        let pml4 = self.pml4 as *const u64;
        // SAFETY: 页表页为本进程分配或内核共享（恒等映射），可读。
        unsafe {
            let e3 = *pml4.add(l4);
            if e3 & P_PRESENT == 0 {
                return None;
            }
            let e2 = *(((e3 & !0xFFF) as *const u64).add(l3));
            if e2 & P_PRESENT == 0 {
                return None;
            }
            let e1 = *(((e2 & !0xFFF) as *const u64).add(l2));
            if e1 & P_PRESENT == 0 {
                return None;
            }
            if e1 & P_HUGE != 0 {
                return Some((e1 & !0xFFFFF) as usize);
            }
            let e0 = *(((e1 & !0xFFF) as *const u64).add(l1));
            if e0 & P_PRESENT == 0 {
                return None;
            }
            Some((e0 & !0xFFF) as usize)
        }
    }
}

/// 分配一页并清零，作为页表中间层。
fn alloc_table() -> usize {
    let p = mm::alloc_pages(0);
    assert!(p != 0, "pt: table alloc failed");
    // SAFETY: 分配的物理页。
    unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096) };
    p
}

/// 统计用户地址空间中 `P_USER` 置位的 4K 页数（RSS 页数，/proc/self/status 用）。
/// 跳过内核恒等映射（PML4[0] 为 supervisor，无 `P_USER`）。
pub fn count_user_pages(pml4: usize) -> usize {
    if pml4 == 0 {
        return 0;
    }
    let mut n = 0usize;
    // SAFETY: pml4 为当前用户进程页表根，恒等映射下可读。
    unsafe {
        for l4 in 0..512usize {
            let e4 = *((pml4 as *const u64).add(l4));
            if e4 & P_PRESENT == 0 || e4 & P_USER == 0 {
                continue; // 未映射或内核映射
            }
            let l3t = (e4 & !0xFFF) as *const u64;
            for l3 in 0..512usize {
                let e3 = *l3t.add(l3);
                if e3 & P_PRESENT == 0 {
                    continue;
                }
                if e3 & P_HUGE != 0 {
                    n += 1;
                    continue;
                }
                let l2t = (e3 & !0xFFF) as *const u64;
                for l2 in 0..512usize {
                    let e2 = *l2t.add(l2);
                    if e2 & P_PRESENT == 0 {
                        continue;
                    }
                    if e2 & P_HUGE != 0 {
                        n += 1;
                        continue;
                    }
                    let l1t = (e2 & !0xFFF) as *const u64;
                    for l1 in 0..512usize {
                        if *l1t.add(l1) & P_PRESENT != 0 {
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    n
}

/// 切换到用户页表并 `iretq` 进入 ring3（不返回）。
///
/// # Safety
/// 仅启动阶段调用一次；此后 CPU 处于用户态，直到用户程序退出。
pub fn enter_user(pml4: usize, entry: u64, rsp: u64) -> ! {
    // SAFETY: 切换 CR3 + iretq 为特权指令；帧参数来自 ELF 加载器。
    unsafe {
        core::arch::asm!("mov cr3, {0}", in(reg) pml4, options(nostack, nomem));
        core::arch::asm!(
            "mov ds, {ds}",
            "mov es, {ds}",
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ds = in(reg) USER_DATA_SEL as u64,
            ss = in(reg) USER_DATA_SEL as u64,
            rsp = in(reg) rsp,
            rflags = in(reg) 0x202u64, // IF=1：允许 PIT 抢占用户态（M6 多任务调度）
            cs = in(reg) USER_CODE_SEL as u64,
            rip = in(reg) entry,
            options(noreturn)
        );
    }
}
