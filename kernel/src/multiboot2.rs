//! multiboot2 启动信息解析（DESIGN.md §1.3：读取内存映射 tag）。
//!
//! bootloader 以 `ebx` 传入 MBI 地址；布局：
//!   偏移 0：total_size (u32) ；偏移 4：reserved (u32) ；偏移 8 起：tag 序列。
//! tag：type (u32) + size (u32) + 数据，8 字节对齐；type=0 为结束 tag。
//! type=6 为内存映射（memory map）：entry_size/entry_version + 条目{base,len,type,reserved}。

use crate::println;

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
