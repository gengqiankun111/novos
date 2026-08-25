//! Novos-OS 内核入口（M0：最小可启动内核 + 串口）。
//!
//! 启动流程见 DESIGN.md §1.3：boot.asm（长模式/页表/GDT）→ `rust_start` →
//! 串口 → IDT/PIC → 打印 multiboot2 内存映射 → 空闲 halt 循环。

#![no_std]
#![no_main]

use novos_kernel::{interrupts, multiboot2, println, serial};

/// multiboot2 规范要求 bootloader 传入的 magic。
const MB2_BOOT_MAGIC: u32 = 0x36D76289;

/// Rust 入口（由 boot.asm 在长模式下调用）。
///
/// # Safety
/// 仅由 boot.asm 调用：此时已开启长模式、分页、SSE，栈已就绪。
#[no_mangle]
pub unsafe extern "C" fn rust_start(magic: u32, mbi_addr: u32) -> ! {
    serial::init();

    if magic != MB2_BOOT_MAGIC {
        novos_kernel::panic_println!("bad multiboot2 magic: {:#x} (expect {:#x})", magic, MB2_BOOT_MAGIC);
        halt_loop();
    }

    interrupts::init();

    println!("Novos-OS: boot ok");
    println!("{}", interrupts::info());
    println!("memory regions:");
    // SAFETY: magic 校验通过，mbi_addr 由 bootloader 保证有效。
    unsafe { multiboot2::print_memory_map(mbi_addr) };

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
