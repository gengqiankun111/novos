//! Novos-OS 内核库（M0：最小可启动内核 + 串口）。
//!
//! 对应 DESIGN.md §1.3 启动流程：boot.asm 完成长模式 + 直接映射页表，
//! 内核库负责串口/GDT/IDT 初始化与内存映射打印。

#![no_std]

core::arch::global_asm!(include_str!("boot.asm"), options(att_syntax));

pub mod interrupts;
pub mod multiboot2;
pub mod port;
pub mod serial;
