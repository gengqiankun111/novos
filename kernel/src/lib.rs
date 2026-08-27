//! 山水观心操作系统内核库（M0：最小可启动内核 + 串口）。
//!
//! 对应 DESIGN.md §1.3 启动流程：boot.asm 完成长模式 + 直接映射页表，
//! 内核库负责串口/GDT/IDT 初始化与内存映射打印。

#![no_std]

extern crate alloc;

// boot.asm：启动全流程（长模式/页表/多核唤醒 stub 等），真实内核与 host 测试
// 均需其中的符号（_start/fork_wrapper/syscall_entry/gdt64/pdpt）。
core::arch::global_asm!(include_str!("boot.asm"), options(att_syntax));
// boot_note.asm：PVH ELF note（`.note.Xen, "a", @note` 为 GNU/ELF 语法，
// host 测试（COFF 目标）汇编器不识别）——仅在真实内核构建（非 test）引入。
#[cfg(not(test))]
core::arch::global_asm!(include_str!("boot_note.asm"), options(att_syntax));

pub mod dcache;
pub mod block;
pub mod elf;
pub mod ext4;
pub mod fs;
pub mod futex;
pub mod gdt;
pub mod interrupts;
pub mod mm;
pub mod multiboot2;
pub mod net;
pub mod page_table;
pub mod pit;
pub mod port;
pub mod rbtree;
pub mod serial;
pub mod smp;
pub mod socket;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod vga;
pub mod vmm;
