//! Novos-OS 内核入口（M0：最小可启动内核 + 串口；M1：接入物理内存 + 内核堆）。
//!
//! 启动流程见 DESIGN.md §1.3：boot.asm（长模式/页表/GDT）→ `rust_start` →
//! 串口/VGA → IDT/PIC → 打印 multiboot2 内存映射 → mm::init + 自测 → 空闲 halt 循环。

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

use novos_kernel::{interrupts, mm, multiboot2, pit, println, serial, sync, task, vga};

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
    vga::init();

    if magic == MB2_BOOT_MAGIC {
        println!("Novos-OS: boot ok (multiboot2)");
    } else if magic == MB1_BOOT_MAGIC {
        println!("Novos-OS: boot ok (multiboot1)");
    } else {
        println!("Novos-OS: boot ok (pvh)");
    }

    // IDT 先行：若后续 mm 初始化出异常，可打印寄存器快照而非静默三重故障。
    interrupts::init();
    println!("{}", interrupts::info());

    // M1：初始化物理内存 + 内核堆（此后 Vec/Box/Arc 可用）。
    // SAFETY: 仅启动时调用一次，早于任何分配。
    unsafe { mm::init() };
    println!(
        "mm: heap {:#x}..{:#x} ({} KiB), managed pages {}",
        mm::MEM_START,
        mm::MEM_END,
        mm::heap_capacity_bytes() / 1024,
        mm::MEM_PAGES
    );

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

    // M1 自测：Buddy 合并 / Slab 复用 / Vec 10000 求和 / Box。
    match mm::self_test() {
        Ok(msg) => println!("mm self-test: {msg}"),
        Err(e) => println!("mm self-test: FAILED: {e}"),
    }
    let stats = mm::mem_stats();
    println!(
        "mm stats: buddy_pages={} slab_pages={} kernel_used={} B free_pages={}",
        stats.buddy_pages,
        stats.slab_pages,
        stats.kernel_used_bytes,
        mm::free_page_count()
    );

    // M2 切片2：定时器 + 同步原语（PIP 演示：L 持锁 / M 中间优先级抢占 / H 高优先级等待）。
    pit::init();
    task::spawn("low", worker_low, 1).expect("spawn low");
    task::spawn("med", worker_med, 2).expect("spawn med");
    task::spawn("high", worker_high, 3).expect("spawn high");
    println!("m2: sync PIP demo (L=1 lock, M=2 busy, H=3 waiter)");

    println!("Novos-OS: init done, entering idle halt loop");
    halt_loop();
}

/// PIP 演示锁。
static LOCK: sync::Mutex = sync::Mutex::new();

/// L（低优先级 1）：拿锁后临界区忙等 10 tick。被 M 抢占会显著推迟；
/// H 等锁时 PIP 提升 L → 临界区后半段不被 M 抢占，准时完成。
fn worker_low() {
    loop {
        task::sleep_ticks(10); // 错开启动，让 H 先睡、M 先跑
        LOCK.lock();
        println!("  [L] lock @t{}", task::ticks());
        let start = task::ticks();
        // 临界区：忙等 10 tick
        while task::ticks() < start + 10 {
            // SAFETY: 忙等。
            unsafe { core::arch::asm!("pause", options(nomem, nostack)) };
        }
        let boosted = task::effective(task::current_id()) > task::priority(task::current_id());
        println!(
            "  [L] unlock @t{} (boosted={})",
            task::ticks(),
            boosted
        );
        LOCK.unlock();
        task::sleep_ticks(60);
    }
}

/// M（中间优先级 2）：睡眠 2 tick + 忙等 6 tick 的周期活动任务。
/// 无 PIP 时会在 L 的临界区内抢占 L；PIP 提升 L(3) 后无法抢占。
fn worker_med() {
    loop {
        let s = task::ticks();
        task::sleep_ticks(2);
        while task::ticks() < s + 8 {
            // SAFETY: 忙等。
            unsafe { core::arch::asm!("pause", options(nomem, nostack)) };
        }
        println!("  [M] busy window done @t{}", task::ticks());
    }
}

/// H（高优先级 3）：等待锁；持锁者被 PIP 提升后快速拿到锁。
fn worker_high() {
    loop {
        task::sleep_ticks(15); // 让 L 先拿到锁
        println!("  [H] try lock @t{}", task::ticks());
        LOCK.lock();
        println!("  [H] got lock @t{}", task::ticks());
        LOCK.unlock();
        println!("  [H] released @t{}", task::ticks());
        task::sleep_ticks(40);
    }
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

/// panic：打印位置 + 消息到串口/VGA，然后停机（DESIGN.md §8.2）。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    novos_kernel::panic_println!("PANIC at {}", info.location().unwrap());
    novos_kernel::panic_println!("message: {:?}", info.message());
    novos_kernel::panic_println!("Novos-OS: halted");
    halt_loop()
}

/// 堆分配失败回调（liballoc 在 `alloc` 返回空后调用）。
#[alloc_error_handler]
fn alloc_error(_layout: core::alloc::Layout) -> ! {
    panic!("mm: out of memory");
}
