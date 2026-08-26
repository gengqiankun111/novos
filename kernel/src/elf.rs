//! M3 切片4：ELF 加载器（静态 ET_EXEC）→ 映射到用户地址空间 → 进入 ring3。
//!
//! 加载对象为嵌入内核镜像的用户态 init/shell（`include_bytes!`），
//! 链接基址 0x80_0000_0000（见 userspace/linker.ld）。流程：
//! 1. 校验 ELF64 头（magic/class/endian/ET_EXEC/x86_64）；
//! 2. 遍历程序头，将每个 PT_LOAD 段逐 4K 页分配物理页、拷贝文件数据、
//!    零填充 bss，映射 USER（读写按 p_flags）到用户页表；
//! 3. 建立用户栈（USER_STACK_VADDR 向下 128KB）并布置最小栈帧；
//! 4. `page_table::enter_user` 经 iretq 进入 ring3（不返回）。

use crate::mm;
use crate::page_table::{UserPageTable, P_PRESENT, P_USER, P_WRITABLE, USER_STACK_VADDR};

/// 嵌入的用户态 init/shell 二进制（Makefile 先构建 userspace 再构建内核，
/// 后续 `cargo build` 会因 `include_bytes!` 自动跟踪该文件变化而重链内核）。
static INIT_ELF: &[u8] =
    include_bytes!("../../userspace/target/x86_64-unknown-none/release/novos-init");

// ---- ELF64 头字段偏移 ----
const EI_CLASS: usize = 4; // 2 = ELF64
const EI_DATA: usize = 5; // 1 = little endian
const E_ENTRY: usize = 24; // u64
const E_TYPE: usize = 16; // u16, 2 = ET_EXEC
const E_MACHINE: usize = 18; // u16, 0x3E = x86_64
const E_PHOFF: usize = 32; // u64
const E_PHENTSIZE: usize = 54; // u16
const E_PHNUM: usize = 56; // u16

// ---- ELF64 程序头（Elf64_Phdr，56 字节）字段偏移 ----
const PT_LOAD: u32 = 1;
const PF_W: u32 = 2;

fn rd16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn rd64(b: &[u8], off: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(v)
}

/// 加载嵌入的用户态 ELF 并进入 ring3 执行（不返回）。
pub fn load_and_run() -> ! {
    let elf = INIT_ELF;
    assert!(elf.len() >= 64, "elf: image too small ({}B)", elf.len());
    assert!(
        &elf[0..4] == b"\x7fELF" && elf[EI_CLASS] == 2 && elf[EI_DATA] == 1,
        "elf: bad magic/class/endian"
    );
    let e_type = rd16(elf, E_TYPE);
    let e_machine = rd16(elf, E_MACHINE);
    assert!(e_type == 2, "elf: not ET_EXEC (type={e_type})");
    assert!(e_machine == 0x3E, "elf: not x86_64 (machine={e_machine:#x})");

    let entry = rd64(elf, E_ENTRY);
    let phoff = rd64(elf, E_PHOFF) as usize;
    let phentsize = rd16(elf, E_PHENTSIZE) as usize;
    let phnum = rd16(elf, E_PHNUM) as usize;
    crate::println!(
        "m3/elf: phoff={phoff} phentsize={phentsize} phnum={phnum} entry={entry:#x}"
    );

    let mut pt = UserPageTable::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let p_type = rd32(elf, ph);
        if p_type != PT_LOAD {
            continue;
        }
        let p_flags = rd32(elf, ph + 4);
        let p_offset = rd64(elf, ph + 8);
        let p_vaddr = rd64(elf, ph + 16);
        let p_filesz = rd64(elf, ph + 32);
        let p_memsz = rd64(elf, ph + 40);
        map_segment(&mut pt, elf, p_vaddr, p_offset, p_filesz, p_memsz, p_flags);
        crate::println!(
            "m3/elf: PT_LOAD vaddr={p_vaddr:#x} filesz={p_filesz:#x} memsz={p_memsz:#x} flags={p_flags:#x}"
        );
    }

    // 用户栈：USER_STACK_VADDR 向下 128KB，全零页。
    // 注意：此刻 CR3 仍是启动页表（恒等映射 0~1GB），用户虚拟地址不可写，
    // 故记录 sp 所在页的物理地址，后续经恒等映射写栈帧。
    const STACK_PAGES: usize = 32;
    let mut sp_phys = 0usize;
    for i in 0..STACK_PAGES {
        let vaddr = USER_STACK_VADDR - ((i as u64 + 1) * 4096);
        let phys = mm::alloc_pages(0);
        assert!(phys != 0, "elf: stack page alloc failed");
        // SAFETY: 分配的物理页。
        unsafe { core::ptr::write_bytes(phys as *mut u8, 0, 4096) };
        pt.map_page(vaddr, phys, P_PRESENT | P_WRITABLE | P_USER);
        if i == 0 {
            sp_phys = phys; // sp = stack_top-16 落在 i=0 的页（0x8001FF000）
        }
    }
    // 最小合法栈帧（Linux ABI）：argv[0]=NULL, argc=0；rsp 保持 16 字节对齐。
    let stack_top = USER_STACK_VADDR as usize;
    let sp = stack_top - 16;
    assert!(sp_phys != 0, "elf: stack phys missing");
    // SAFETY: sp_phys 为刚分配且清零的物理页；恒等映射下可直接写。
    unsafe {
        *(sp_phys as *mut u64) = 0; // argc = 0
        *((sp_phys + 8) as *mut u64) = 0; // argv[0] = NULL
    }
    crate::println!(
        "m3/elf: user stack {stack_top:#x} sp={sp:#x}, entering ring3..."
    );
    // M6-切片1：注册用户任务（CR3 + 根 pid），使 fork/调度可用。
    crate::task::register_user_task(pt.pml4);
    crate::page_table::enter_user(pt.pml4, entry, sp as u64)
}

/// 映射一个 PT_LOAD 段：按 4K 页映射，拷贝文件数据，零填充其余。
///
/// 相邻段可能因未对齐边界共享同一页（如 .text 与 .rodata 相邻）：
/// 若页面已映射则合并写入既有页，不重复分配/覆盖。
fn map_segment(
    pt: &mut UserPageTable,
    elf: &[u8],
    vaddr: u64,
    offset: u64,
    filesz: u64,
    memsz: u64,
    flags: u32,
) {
    let start = vaddr;
    let end = vaddr + memsz;
    let mut page = vaddr & !0xFFF;
    while page < end {
        // 本页与文件数据重叠区间 [lo, hi)（虚拟地址）。
        let lo = core::cmp::max(page, start);
        let hi = core::cmp::min(page + 4096, start + filesz);
        // 已映射（相邻段共享页）→ 复用既有物理页；否则分配新页并映射。
        let phys = match pt.translate(page) {
            Some(p) => p,
            None => {
                let p = mm::alloc_pages(0);
                assert!(p != 0, "elf: segment page alloc failed");
                // SAFETY: 分配的物理页，先清零（bss 区与未对齐首尾页）。
                unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096) };
                let mut pte = P_PRESENT | P_USER;
                if flags & PF_W != 0 {
                    pte |= P_WRITABLE;
                }
                pt.map_page(page, p, pte);
                p
            }
        };
        if lo < hi {
            // SAFETY: 重叠区间落在 ELF 镜像内；目标页已清零/既有页可写。
            unsafe {
                let dst = (phys + (lo - page) as usize) as *mut u8;
                let src = elf.as_ptr().add(offset as usize + (lo - start) as usize);
                core::ptr::copy_nonoverlapping(src, dst, (hi - lo) as usize);
            }
        }
        // 共享页升级：段要求可写而既有映射为只读时，补上写位。
        if flags & PF_W != 0 {
            pt.map_page(page, phys, P_PRESENT | P_USER | P_WRITABLE);
        }
        page += 4096;
    }
}
