//! M3 切片4 + M11 切片5：ELF 加载器（静态 ET_EXEC / 动态判定）→ 用户地址空间 → ring3。
//!
//! 加载对象为嵌入内核镜像的用户态 init/shell（`include_bytes!`），
//! 链接基址 0x80_0000_0000（见 userspace/linker.ld）。流程：
//! 1. `inspect` 校验 ELF64 头并解析程序头：PT_LOAD（映射）、PT_INTERP（解释器）、
//!    PT_DYNAMIC + DT_NEEDED（动态库依赖，M11-切片5）；
//! 2. 遍历 PT_LOAD 段逐 4K 页分配物理页、拷贝文件数据、零填充 bss，映射 USER 页表；
//! 3. 建立用户栈并布置 Linux ABI 帧：argc/argv/envp + 辅助向量
//!    （AT_PHDR/AT_PHENT/AT_PHNUM/AT_PAGESZ/AT_BASE/AT_ENTRY/AT_RANDOM/AT_EXECFN）；
//! 4. `page_table::enter_user` 经 iretq 进入 ring3（不返回）。

use alloc::vec::Vec;
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
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PF_W: u32 = 2;

// ---- 动态段条目（Elf64_Dyn，16 字节）d_tag ----
const DT_NULL: i64 = 0;
const DT_NEEDED: i64 = 1;
const DT_STRTAB: i64 = 5;

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

/// ELF 解析结果（M11-切片5：动态段/解释器/依赖识别）。
pub struct ElfInfo<'a> {
    /// e_type：2 = ET_EXEC，3 = ET_DYN。
    pub e_type: u16,
    /// e_entry（程序入口虚拟地址）。
    pub entry: u64,
    /// 程序头表文件偏移。
    pub phoff: u64,
    /// 单个程序头大小（标准 56）。
    pub phentsize: u16,
    /// 程序头个数。
    pub phnum: u16,
    /// 程序头表虚拟地址（首个 PT_LOAD p_vaddr + phoff，供 AT_PHDR）。
    pub phdr_vaddr: u64,
    /// PT_INTERP 解释器路径（静态 ELF 为 None）。
    pub interp: Option<&'a str>,
    /// PT_DYNAMIC 段虚拟地址（静态 ELF 为 None）。
    pub dyn_vaddr: Option<u64>,
    /// DT_NEEDED 动态库依赖名列表。
    pub needed: Vec<&'a str>,
}

/// 解析 ELF64 头 + 程序头（PT_LOAD/PT_INTERP/PT_DYNAMIC）与动态依赖（DT_NEEDED）。
pub fn inspect<'a>(elf: &'a [u8]) -> Result<ElfInfo<'a>, &'static str> {
    if elf.len() < 64 {
        return Err("elf: too small");
    }
    if &elf[0..4] != b"\x7fELF" || elf[EI_CLASS] != 2 || elf[EI_DATA] != 1 {
        return Err("elf: bad magic/class/endian");
    }
    let e_type = rd16(elf, E_TYPE);
    if e_type != 2 && e_type != 3 {
        return Err("elf: not ET_EXEC/ET_DYN");
    }
    if rd16(elf, E_MACHINE) != 0x3E {
        return Err("elf: not x86_64");
    }
    let entry = rd64(elf, E_ENTRY);
    let phoff = rd64(elf, E_PHOFF) as usize;
    let phentsize = rd16(elf, E_PHENTSIZE) as usize;
    let phnum = rd16(elf, E_PHNUM) as usize;
    let mut loads: Vec<(u64, u64, u64)> = Vec::new(); // (p_vaddr, p_offset, p_filesz)
    let mut interp: Option<&'a str> = None;
    let mut dyn_vaddr: Option<u64> = None;
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if ph + 56 > elf.len() {
            break;
        }
        let p_type = rd32(elf, ph);
        let p_offset = rd64(elf, ph + 8);
        let p_vaddr = rd64(elf, ph + 16);
        let p_filesz = rd64(elf, ph + 32);
        match p_type {
            PT_LOAD => loads.push((p_vaddr, p_offset, p_filesz)),
            PT_INTERP => {
                let off = p_offset as usize;
                let max = core::cmp::min(p_filesz as usize, 128);
                if off + max <= elf.len() {
                    let end = elf[off..off + max]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(max);
                    interp = Some(core::str::from_utf8(&elf[off..off + end]).unwrap_or(""));
                }
            }
            PT_DYNAMIC => dyn_vaddr = Some(p_vaddr),
            _ => {}
        }
    }
    // phdr 虚拟地址：程序头表落在文件偏移 [phoff, phoff+phnum*entsize)，
    // 找到覆盖 phoff 的 PT_LOAD，虚拟地址 = p_vaddr + (phoff - p_offset)。
    // 若无段覆盖（如首段 p_offset != 0，ELF 头未包含进段），按 Linux 惯例
    // ELF 头页位于首段 vaddr - p_offset 处（load bias 映射）。
    let phdr_vaddr = match loads
        .iter()
        .find(|&&(_, po, pf)| (phoff as u64) >= po && (phoff as u64) - po < pf)
    {
        Some(&(pv, po, _)) => pv + (phoff as u64 - po),
        None => loads
            .first()
            .map(|&(pv, po, _)| pv.saturating_sub(po) + phoff as u64)
            .unwrap_or(0),
    };
    // DT_NEEDED：经 DT_STRTAB 解析依赖名（strtab 虚拟地址 → 文件偏移）
    let mut needed = Vec::new();
    if let Some(dyn_v) = dyn_vaddr {
        if let Some(dyn_off) = file_offset(elf, dyn_v, &loads) {
            // 第一遍：找 DT_STRTAB
            let mut strtab_v = 0u64;
            let mut i = dyn_off;
            while i + 16 <= elf.len() {
                let tag = rd64(elf, i) as i64;
                let val = rd64(elf, i + 8);
                if tag == DT_NULL {
                    break;
                }
                if tag == DT_STRTAB {
                    strtab_v = val;
                }
                i += 16;
            }
            // 第二遍：收集 DT_NEEDED（d_val = strtab 内偏移）
            if strtab_v != 0 {
                if let Some(strtab_off) = file_offset(elf, strtab_v, &loads) {
                    let mut j = dyn_off;
                    while j + 16 <= elf.len() {
                        let tag = rd64(elf, j) as i64;
                        let val = rd64(elf, j + 8);
                        if tag == DT_NULL {
                            break;
                        }
                        if tag == DT_NEEDED {
                            let so = strtab_off + val as usize;
                            if so < elf.len() {
                                let end = elf[so..]
                                    .iter()
                                    .position(|&b| b == 0)
                                    .unwrap_or(64);
                                needed.push(core::str::from_utf8(&elf[so..so + end]).unwrap_or("?"));
                            }
                        }
                        j += 16;
                    }
                }
            }
        }
    }
    Ok(ElfInfo {
        e_type,
        entry,
        phoff: phoff as u64,
        phentsize: phentsize as u16,
        phnum: phnum as u16,
        phdr_vaddr,
        interp,
        dyn_vaddr,
        needed,
    })
}

/// 虚拟地址 → 文件偏移（在某个 PT_LOAD 段内）。
fn file_offset(elf: &[u8], vaddr: u64, loads: &[(u64, u64, u64)]) -> Option<usize> {
    let _ = elf;
    for &(pv, po, pf) in loads {
        if vaddr >= pv && vaddr - pv < pf {
            return Some((po + (vaddr - pv)) as usize);
        }
    }
    None
}

/// 加载嵌入的用户态 ELF 并进入 ring3 执行（不返回）。
pub fn load_and_run() -> ! {
    let elf = INIT_ELF;
    let info = match inspect(elf) {
        Ok(i) => i,
        Err(e) => panic!("elf: init parse failed: {e}"),
    };
    crate::println!(
        "m3/elf: type={} entry={:#x} phdr={:#x} phnum={}",
        info.e_type, info.entry, info.phdr_vaddr, info.phnum
    );
    match info.interp {
        Some(p) => crate::println!("m3/elf: PT_INTERP {p}"),
        None => crate::println!("m3/elf: static (no PT_INTERP)"),
    }
    match info.dyn_vaddr {
        Some(v) => crate::println!("m3/elf: PT_DYNAMIC {v:#x}"),
        None => crate::println!("m3/elf: no PT_DYNAMIC (static)"),
    }

    let mut pt = UserPageTable::new();
    let phoff = info.phoff as usize;
    let phentsize = info.phentsize as usize;
    for i in 0..info.phnum as usize {
        let ph = phoff + i * phentsize;
        if ph + 56 > elf.len() {
            break;
        }
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
            sp_phys = phys; // sp 落在 i=0 的页（栈顶向下 4KB 内）
        }
    }
    // Linux ABI 启动帧：argc/argv/envp + 辅助向量（M11-切片5）。
    let sp = build_abi_frame(sp_phys, &info);
    crate::println!(
        "m3/elf: user stack {:#x} sp={sp:#x}, auxv phdr={:#x} base=0 entry={:#x}",
        USER_STACK_VADDR,
        info.phdr_vaddr,
        info.entry
    );
    // M6-切片1：注册用户任务（CR3 + 根 pid），使 fork/调度可用。
    crate::task::register_user_task(pt.pml4);
    crate::page_table::enter_user(pt.pml4, info.entry, sp as u64)
}

// ---- 辅助向量（Linux x86_64 ABI，M11-切片5）----
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;
const AT_RANDOM: u64 = 25;
const AT_EXECFN: u64 = 31;
const AT_NULL: u64 = 0;

/// 在用户栈顶页（sp_phys，恒等映射）布置 Linux 启动帧：
/// argc=0 / argv[0]=NULL / envp[0]=NULL / auxv 表 / AT_NULL，
/// 以及 AT_RANDOM(16B) 与 AT_EXECFN 字符串（帧之后的高地址侧）。
/// 返回最终用户 rsp（16 字节对齐）。
fn build_abi_frame(sp_phys: usize, info: &ElfInfo) -> usize {
    const STACK_TOP: usize = USER_STACK_VADDR as usize;
    const PAGE0_V: usize = STACK_TOP - 4096; // 含 sp 的栈页虚拟基址
    // 恒等映射写辅助：虚拟地址 → 物理地址（同页内偏移）。
    let v2p = |v: usize| sp_phys + (v - PAGE0_V);

    // 预计算 auxv（AT_RANDOM/AT_EXECFN 指向帧后数据，需先定帧长）
    let mut auxv: [(u64, u64); 12] = [(0, 0); 12];
    let mut n = 0usize;
    auxv[n] = (AT_PHDR, info.phdr_vaddr);
    n += 1;
    auxv[n] = (AT_PHENT, 56);
    n += 1;
    auxv[n] = (AT_PHNUM, info.phnum as u64);
    n += 1;
    auxv[n] = (AT_PAGESZ, 4096);
    n += 1;
    auxv[n] = (AT_BASE, 0); // 静态 init：无解释器 → 0
    n += 1;
    auxv[n] = (AT_ENTRY, info.entry);
    n += 1;
    // 总条目 = 现有 6 + AT_RANDOM + AT_EXECFN + AT_NULL = 9
    let total = n + 3;
    let frame_end = PAGE0_V + 24 + total * 16; // sp(24B 头部) + auxv 表（含 AT_NULL）
    let rand_v = frame_end; // AT_RANDOM 数据（16B）
    let execfn_v = rand_v + 16; // AT_EXECFN 字符串
    auxv[n] = (AT_RANDOM, rand_v as u64);
    n += 1;
    auxv[n] = (AT_EXECFN, execfn_v as u64);
    n += 1;
    auxv[n] = (AT_NULL, 0);
    n += 1;

    // SAFETY: 栈页刚分配清零且恒等映射；写入值均为栈内合法地址/常量。
    unsafe {
        let put = |v: usize, val: u64| {
            core::ptr::write_volatile(v2p(v) as *mut u64, val);
        };
        // rsp 起点：argc（16 字节对齐）
        let sp = PAGE0_V;
        put(sp, 0); // argc = 0
        put(sp + 8, 0); // argv[0] = NULL
        put(sp + 16, 0); // envp[0] = NULL
        let mut p = sp + 24;
        for &(tag, val) in &auxv[..n] {
            put(p, tag);
            put(p + 8, val);
            p += 16;
        }
        // AT_RANDOM：固定 16 字节（真实熵源见 M12-03，此处标记简化）
        const RANDOM: [u8; 16] = [
            0x9a, 0x5d, 0x4b, 0x31, 0x28, 0xc2, 0x77, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88,
        ];
        core::ptr::copy_nonoverlapping(RANDOM.as_ptr(), v2p(rand_v) as *mut u8, 16);
        // AT_EXECFN：可执行文件路径
        let exe = b"/init\0";
        core::ptr::copy_nonoverlapping(exe.as_ptr(), v2p(execfn_v) as *mut u8, exe.len());
        sp
    }
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

// ---- M11-切片5 自测：合成 ELF 的判定与解析 ----

/// 合成 ELF64 头。
fn put_hdr(b: &mut [u8], e_type: u16, phoff: u64, phnum: u16) {
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 2; // ELF64
    b[5] = 1; // little endian
    b[6] = 1; // EV_CURRENT
    b[16..18].copy_from_slice(&e_type.to_le_bytes());
    b[18..20].copy_from_slice(&0x3E_u16.to_le_bytes()); // x86_64
    b[24..32].copy_from_slice(&0x1000_u64.to_le_bytes()); // entry
    b[32..40].copy_from_slice(&phoff.to_le_bytes());
    b[54..56].copy_from_slice(&56_u16.to_le_bytes()); // phentsize
    b[56..58].copy_from_slice(&phnum.to_le_bytes());
}

/// 合成 Elf64_Phdr。
fn put_phdr(
    b: &mut [u8],
    off: usize,
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
) {
    b[off..off + 4].copy_from_slice(&p_type.to_le_bytes());
    b[off + 4..off + 8].copy_from_slice(&p_flags.to_le_bytes());
    b[off + 8..off + 16].copy_from_slice(&p_offset.to_le_bytes());
    b[off + 16..off + 24].copy_from_slice(&p_vaddr.to_le_bytes());
    b[off + 32..off + 40].copy_from_slice(&p_filesz.to_le_bytes());
    b[off + 40..off + 48].copy_from_slice(&p_memsz.to_le_bytes());
}

/// 静态 ET_EXEC：1 个 PT_LOAD，无 interp/dynamic。
fn mk_static_elf() -> [u8; 200] {
    let mut b = [0u8; 200];
    put_hdr(&mut b, 2, 64, 1);
    put_phdr(&mut b, 64, PT_LOAD, 5, 0, 0x80_0000_0000, 200, 200);
    b
}

/// 动态 ELF：PT_LOAD + PT_INTERP + PT_DYNAMIC（扁平布局，vaddr = BASE + 文件偏移）。
/// `with_interp=false` 生成无 PT_INTERP 的 DSO 形态。
fn mk_dynamic_elf(with_interp: bool) -> [u8; 380] {
    const BASE: u64 = 0x40_0000;
    let mut b = [0u8; 380];
    let phnum = if with_interp { 3 } else { 2 };
    put_hdr(&mut b, 3, 64, phnum); // ET_DYN
    // phdr[0] PT_LOAD：覆盖整个镜像
    put_phdr(&mut b, 64, PT_LOAD, 5, 0, BASE, 380, 380);
    // phdr[1] PT_INTERP（可选）
    let interp_off = 340usize;
    if with_interp {
        put_phdr(&mut b, 120, PT_INTERP, 4, interp_off as u64, 0, 27, 27);
    }
    // phdr[2]（或无 interp 时 phdr[1]）PT_DYNAMIC
    let dyn_off = if with_interp { 176 } else { 120 };
    put_phdr(&mut b, dyn_off, PT_DYNAMIC, 6, 232, BASE + 232, 64, 64);
    // dyn 条目：DT_STRTAB + 2×DT_NEEDED + DT_NULL（各 16B）
    let strtab_off = 296u64;
    let dyn_entries: [(i64, u64); 4] = [
        (DT_STRTAB, BASE + strtab_off),
        (DT_NEEDED, 0), // "libc.so"
        (DT_NEEDED, 8), // "libpthread.so"（"libc.so\0" = 8 字节）
        (DT_NULL, 0),
    ];
    for (i, (tag, val)) in dyn_entries.iter().enumerate() {
        let o = 232 + i * 16;
        b[o..o + 8].copy_from_slice(&tag.to_le_bytes());
        b[o + 8..o + 16].copy_from_slice(&val.to_le_bytes());
    }
    // strtab："libc.so\0libpthread.so\0"
    b[strtab_off as usize..strtab_off as usize + 22].copy_from_slice(b"libc.so\0libpthread.so\0");
    // interp 字符串
    if with_interp {
        b[interp_off..interp_off + 27].copy_from_slice(b"/novos/ld-musl-x86_64.so.1\0");
    }
    b
}

/// ELF 解析自测（M11-切片5）：静态/动态+interp/动态无 interp/坏 magic 四种判定。
pub fn self_test() {
    let mut ok = true;
    let mut check = |cond: bool, label: &str, ok: &mut bool| {
        if cond {
            crate::println!("elf/dyn: {label} ok");
        } else {
            crate::println!("elf/dyn: {label} FAIL");
            *ok = false;
        }
    };
    // 1) 静态 ET_EXEC
    let s = mk_static_elf();
    match inspect(&s) {
        Ok(i) => {
            check(
                i.e_type == 2 && i.interp.is_none() && i.dyn_vaddr.is_none() && i.needed.is_empty(),
                "static ET_EXEC",
                &mut ok,
            );
            if i.e_type == 2 {
                crate::println!("elf/dyn:   entry={:#x} phdr={:#x}", i.entry, i.phdr_vaddr);
            }
        }
        Err(e) => {
            crate::println!("elf/dyn: static ET_EXEC FAIL: {e}");
            ok = false;
        }
    }
    // 2) 动态 + PT_INTERP + DT_NEEDED
    let d = mk_dynamic_elf(true);
    match inspect(&d) {
        Ok(i) => check(
            i.e_type == 3
                && i.interp == Some("/novos/ld-musl-x86_64.so.1")
                && i.dyn_vaddr.is_some()
                && i.needed.len() == 2
                && i.needed[0] == "libc.so"
                && i.needed[1] == "libpthread.so",
            "dynamic + interp + DT_NEEDED",
            &mut ok,
        ),
        Err(e) => {
            crate::println!("elf/dyn: dynamic FAIL: {e}");
            ok = false;
        }
    }
    // 3) 动态无 interp（DSO）
    let d2 = mk_dynamic_elf(false);
    match inspect(&d2) {
        Ok(i) => check(
            i.e_type == 3 && i.interp.is_none() && i.dyn_vaddr.is_some(),
            "dynamic no-interp",
            &mut ok,
        ),
        Err(e) => {
            crate::println!("elf/dyn: dynamic-no-interp FAIL: {e}");
            ok = false;
        }
    }
    // 4) 坏 magic 拒绝
    check(inspect(b"not an elf at all............").is_err(), "bad-magic rejected", &mut ok);
    if ok {
        crate::println!("elf/dyn: self-test PASS");
    } else {
        crate::println!("elf/dyn: self-test FAIL");
    }
}
