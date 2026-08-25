//! Novos-OS 内核入口（M0：最小可启动内核 + 串口；M1：接入物理内存 + 内核堆）。
//!
//! 启动流程见 DESIGN.md §1.3：boot.asm（长模式/页表/GDT）→ `rust_start` →
//! 串口/VGA → IDT/PIC → 打印 multiboot2 内存映射 → mm::init + 自测 → 空闲 halt 循环。

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

use novos_kernel::{interrupts, mm, multiboot2, pit, println, serial, task, vga};

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

    // M2 切片4：定时器 + CFS vruntime 红黑树调度（权重 = 1 << prio，
    // 期望 CPU 占比 A:B:C ≈ 2:4:8）+ per_cpu 占位。
    pit::init();
    task::spawn("cfs-a", worker_cfs_a, 1).expect("spawn cfs-a");
    task::spawn("cfs-b", worker_cfs_b, 2).expect("spawn cfs-b");
    task::spawn("cfs-c", worker_cfs_c, 3).expect("spawn cfs-c");
    // SMP 预热：验证 cpu_rq(0) 可访问（占位）
    let rq = unsafe { novos_kernel::smp::cpu_rq(0) };
    println!(
        "m2: cfs demo (rq tree empty={}) started",
        rq.rbt.is_empty()
    );

    println!("Novos-OS: init done, entering idle halt loop");
    halt_loop();
}

/// CFS 忙等任务公共体：每 100 tick 打印本任务的 run_ticks 与 vruntime。
fn cfs_loop(tag: &'static str, prio: u8) {
    let mut last = 0u64;
    loop {
        let t = task::ticks();
        if t >= last + 100 {
            last = t;
            let id = task::current_id();
            println!(
                "  [{}] prio={} run={} vr={}",
                tag,
                prio,
                task::run_ticks(id),
                task::vruntime(id)
            );
        }
        // SAFETY: 忙等。
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
}

fn worker_cfs_a() {
    cfs_loop("A", 1);
}
fn worker_cfs_b() {
    cfs_loop("B", 2);
}
fn worker_cfs_c() {
    cfs_loop("C", 3);
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
