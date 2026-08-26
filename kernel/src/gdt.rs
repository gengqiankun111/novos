//! M3 切片1：GDT/TSS 初始化——加载 TSS（用户态切换到内核时的 RSP0）+ 重载段寄存器。
//!
//! boot.asm 已扩展 GDT：0x08=kcode, 0x10=kdata, 0x18=udata, 0x20=ucode, 0x28=TSS。
//! 此处用 `ltr` 加载 TSS（64-bit 描述符已由 boot.asm 声明），并把 RSP0 指向 syscall 栈。
//! 用户态发生异常/中断时，CPU 依 TSS.RSP0 切换到内核栈。

use core::arch::asm;

/// 用户态代码段选择子（RPL3）。
pub const USER_CODE_SEL: u16 = 0x20 | 3;
/// 用户态数据段选择子（RPL3）。
pub const USER_DATA_SEL: u16 = 0x18 | 3;
/// 内核代码段选择子。
pub const KERNEL_CODE_SEL: u16 = 0x08;
/// 内核数据段选择子。
pub const KERNEL_DATA_SEL: u16 = 0x10;
/// TSS 段选择子。
pub const TSS_SEL: u16 = 0x28;

// boot.asm 导出的符号。
extern "C" {
    static syscall_stack_top: u8;
    static tss_rsp0: u64;
    static gdt64: u8;
    static tss: u8;
}

/// TSS 描述符在 GDT 中的偏移（index 5 × 8）。
const TSS_DESC_OFF: usize = 5 * 8;

/// 加载 TSS：先填充 GDT 中 TSS 描述符的 base 字段，再设置 RSP0，最后 `ltr`。
///
/// # Safety
/// 仅启动阶段调用一次；`ltr` 为特权指令。
pub fn init() {
    // SAFETY: gdt64/tss 由 boot.asm 静态分配，TSS 描述符 base 字段为占位 0。
    unsafe {
        let tss_addr = core::ptr::addr_of!(tss) as u64;
        let desc = core::ptr::addr_of!(gdt64) as *mut u8;
        let d = desc.add(TSS_DESC_OFF);
        // base[15:0] @ +2, base[23:16] @ +4, base[31:24] @ +7, base[63:32] @ +8
        core::ptr::write_volatile(d.add(2) as *mut u16, (tss_addr & 0xFFFF) as u16);
        core::ptr::write_volatile(d.add(4), ((tss_addr >> 16) & 0xFF) as u8);
        core::ptr::write_volatile(d.add(7), ((tss_addr >> 24) & 0xFF) as u8);
        core::ptr::write_volatile(d.add(8) as *mut u32, (tss_addr >> 32) as u32);

        // RSP0 = syscall 内核栈顶
        let rsp0_ptr = core::ptr::addr_of!(tss_rsp0) as *mut u64;
        *rsp0_ptr = core::ptr::addr_of!(syscall_stack_top) as u64;
    }
    // SAFETY: 加载 TSS 段选择子（0x28 已在 GDT 中，boot.asm 声明）。
    unsafe {
        asm!("ltr {0:x}", in(reg) TSS_SEL, options(nomem, nostack));
    }
}

/// 供启动日志确认 TSS 已加载。
pub fn info() -> &'static str {
    "gdt(user/tss)/ltr ready"
}
