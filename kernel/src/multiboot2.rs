//! multiboot2 启动信息解析（DESIGN.md §1.3：读取内存映射 tag）。
//!
//! bootloader 以 `ebx` 传入 MBI 地址；布局：
//!   偏移 0：total_size (u32) ；偏移 4：reserved (u32) ；偏移 8 起：tag 序列。
//! tag：type (u32) + size (u32) + 数据，8 字节对齐；type=0 为结束 tag。
//! type=6 为内存映射（memory map）：entry_size/entry_version + 条目{base,len,type,reserved}。

use crate::println;

/// PVH 启动信息魔数（hvm_start_info.magic）。
pub const PVH_START_INFO_MAGIC: u32 = 0x336EC578;

/// 内存区域类型（对应 multiboot2 memory map entry type）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionType {
    Available,
    Reserved,
    AcpiReclaimable,
    AcpiNvs,
    BadRam,
    Other(u32),
}

/// 解析并打印 bootloader 传入的内存映射。
///
/// # Safety
/// `mbi_addr` 必须由 bootloader 通过 multiboot2 协议保证为合法映射地址。
pub unsafe fn print_memory_map(mbi_addr: u32) {
    let base = mbi_addr as *const u8;
    let total_size = unsafe { *(base as *const u32) };
    let end = unsafe { base.add(total_size as usize) };

    let mut tag_ptr = unsafe { base.add(8) };
    let mut region_count = 0usize;

    // SAFETY: mbi 由 bootloader 保证有效；tag 8 字节对齐、total_size 界内。
    unsafe {
        while tag_ptr < end {
            let tag_type = *(tag_ptr as *const u32);
            let tag_size = *(tag_ptr.add(4) as *const u32);
            if tag_type == 0 {
                break; // end tag
            }
            if tag_type == 6 {
                let entry_size = *(tag_ptr.add(8) as *const u32);
                let mut entry = tag_ptr.add(16);
                let tag_end = tag_ptr.add(tag_size as usize);
                while entry < tag_end {
                    let base_addr = u64::from(*(entry as *const u32))
                        | (u64::from(*(entry.add(4) as *const u32)) << 32);
                    let length = u64::from(*(entry.add(8) as *const u32))
                        | (u64::from(*(entry.add(12) as *const u32)) << 32);
                    let mtype = *(entry.add(16) as *const u32);
                    let kind = match mtype {
                        1 => RegionType::Available,
                        3 => RegionType::AcpiReclaimable,
                        4 => RegionType::AcpiNvs,
                        5 => RegionType::BadRam,
                        other => RegionType::Other(other),
                    };
                    println!(
                        "  {:#018x}..{:#018x} ({:?})",
                        base_addr,
                        base_addr + length,
                        kind
                    );
                    region_count += 1;
                    entry = entry.add(entry_size as usize);
                }
            }
            tag_ptr = tag_ptr.add(tag_size as usize);
        }
    }

    println!("memory regions: {} entries", region_count);
}

/// PVH（QEMU `-kernel`）启动信息解析：`hvm_start_info` + E820 内存映射。
///
/// 布局（Xen HVM 约定）：
/// ```c
/// struct hvm_start_info {
///   u32 magic; u32 flags; u32 nr_modules; u32 mods_paddr;
///   u32 nr_memmap_entries; u32 memmap_paddr;
///   u32 rsdp_paddr; u32 cmdline_paddr;
/// };
/// struct hvm_memmap_table_entry { u64 addr; u64 size; u32 type; u32 reserved; };
/// ```
///
/// # Safety
/// `info_addr` 由 QEMU 在 PVH 入口通过 ebx 传入，保证为合法映射地址。
pub unsafe fn print_pvh_memory_map(info_addr: u32) -> bool {
    let info = info_addr as *const u8;
    // SAFETY: PVH 协议保证 info_addr 有效。
    let magic = unsafe { *(info as *const u32) };
    if magic != PVH_START_INFO_MAGIC {
        println!("  pvh: bad magic {:#x}", magic);
        return false;
    }
    let nr_entries = unsafe { *(info.add(16) as *const u32) };
    let memmap = unsafe { *(info.add(20) as *const u32) };
    // SAFETY: PVH 协议保证 memmap 数组有效且 nr_entries 一致。
    unsafe {
        for i in 0..nr_entries {
            let e = (memmap as usize + i as usize * 24) as *const u8;
            let addr = u64::from(*(e as *const u32)) | (u64::from(*(e.add(4) as *const u32)) << 32);
            let size = u64::from(*(e.add(8) as *const u32)) | (u64::from(*(e.add(12) as *const u32)) << 32);
            let mtype = *(e.add(16) as *const u32);
            let kind = match mtype {
                1 => RegionType::Available,
                3 => RegionType::AcpiReclaimable,
                4 => RegionType::AcpiNvs,
                5 => RegionType::BadRam,
                other => RegionType::Other(other),
            };
            println!("  {:#018x}..{:#018x} ({:?})", addr, addr + size, kind);
        }
    }
    println!("memory regions: {} entries (pvh)", nr_entries);
    true
}

/// multiboot1 启动信息解析（QEMU 扁平镜像）：打印 mem_lower/mem_upper（KB）。
///
/// # Safety
/// `info_addr` 由 bootloader 在 multiboot1 协议中通过 ebx 传入，保证有效。
pub unsafe fn print_mb1_info(info_addr: u32) {
    let info = info_addr as *const u8;
    // SAFETY: mb1 协议保证 info 结构有效。
    let flags = unsafe { *(info as *const u32) };
    if flags & 0x1 == 0 {
        println!("  mb1: meminfo flag not set, skipping");
        return;
    }
    let mem_lower = unsafe { *(info.add(4) as *const u32) }; // KB
    let mem_upper = unsafe { *(info.add(8) as *const u32) }; // KB
    println!("  low memory: {} KB", mem_lower);
    println!(
        "  high memory: {} KB (1MB .. {:#x})",
        mem_upper,
        0x100000usize + mem_upper as usize * 1024
    );
    println!("memory regions: mb1 summary (qemu flat)");
}
