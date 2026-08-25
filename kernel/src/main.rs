//! Novos-OS 内核入口（M0：最小可启动内核 + 串口）。
//!
//! 启动流程见 DESIGN.md §1.3：boot.asm（长模式/页表/GDT）→ `rust_start` →
//! 串口 → IDT/PIC → 打印 multiboot2 内存映射 → 空闲 halt 循环。

#![no_std]
#![no_main]

use novos_kernel::{interrupts, multiboot2, println, serial};

/// multiboot2 规范要求 bootloader 传入的 magic。
const MB2_BOOT_MAGIC: u32 = 0x36D76289;
/// multiboot1 规范要求 bootloader 传入的 magic（QEMU 扁平镜像）。
const MB1_BOOT_MAGIC: u32 = 0x2BADB002;

/// Rust 入口（由 boot.asm 在长模式下调用）。
///
/// boot.asm 兼容两种启动协议，统一以 `(magic, info)` 传入：
/// - multiboot2（GRUB）：`magic == 0x36D76289`，`info` = mbi 地址；
/// - PVH（QEMU `-kernel`）：`magic` 未定义，`info` = `&hvm_start_info`（ebx）。
///
/// # Safety
/// 仅由 boot.asm 调用：此时已开启长模式、分页、SSE，栈已就绪。
#[no_mangle]
pub unsafe extern "C" fn rust_start(magic: u32, info_addr: u32) -> ! {
    serial::init();

    if magic == MB2_BOOT_MAGIC {
        println!("Novos-OS: boot ok (multiboot2)");
    } else if magic == MB1_BOOT_MAGIC {
        println!("Novos-OS: boot ok (multiboot1)");
    } else {
        println!("Novos-OS: boot ok (pvh)");
    }

    interrupts::init();
    println!("{}", interrupts::info());
    println!("memory regions:");
    if magic == MB2_BOOT_MAGIC {
        // SAFETY: multiboot2 magic 校验通过，info_addr 由 bootloader 保证有效。
        unsafe { multiboot2::print_memory_map(info_addr) };
    } else if magic == MB1_BOOT_MAGIC {
        // SAFETY: multiboot1 magic 校验通过，info_addr 由 bootloader 保证有效。
        unsafe { multiboot2::print_mb1_info(info_addr) };
    } else {
        // SAFETY: PVH 协议保证 info_addr 指向 hvm_start_info。
        unsafe { multiboot2::print_pvh_memory_map(info_addr) };
    }

    println!("Novos-OS: init done, entering idle halt loop");
    halt_loop();
}

/// 空闲停机循环。
pub fn halt_loop() -> ! {
    loop {
        // SAFETY: hlt 为特权指令，无内存副作用。
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// panic：打印位置 + 消息到串口，然后停机（DESIGN.md §8.2）。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    novos_kernel::panic_println!("PANIC at {}", info.location().unwrap());
    novos_kernel::panic_println!("message: {:?}", info.message());
    novos_kernel::panic_println!("Novos-OS: halted");
    halt_loop()
}
