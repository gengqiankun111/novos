//! M3 切片3：用户态页表 + 进入 ring3。
//!
//! 构建独立用户进程 4 级页表（PML4[0]=内核恒等映射 supervisor，PML4[1]=用户映射 USER），
//! 经 `iretq` 切换到 ring3，运行一段手写用户态机器码（`syscall` 输出 "hello from userspace"）。
//!
//! 地址布局（M3 简化，非标准高/低半区分离）：
//! - 用户代码：`0x80_0000_0000`（512GB，PML4[1]）
//! - 用户栈：`0x80_0020_0000`（+2MB）
//! - 内核恒等映射：0~1GB（PML4[0]，supervisor，syscall 进入后内核可访问）

use crate::gdt::{USER_CODE_SEL, USER_DATA_SEL};
use crate::mm;

/// 页表项标志。
const P_PRESENT: u64 = 1 << 0;
const P_WRITABLE: u64 = 1 << 1;
const P_USER: u64 = 1 << 2;
const P_HUGE: u64 = 1 << 7; // 2MB 大页（PS）

/// 用户代码虚拟地址（512GB）。
pub const USER_CODE_VADDR: u64 = 0x80_0000_0000;
/// 用户栈虚拟地址（512GB + 2MB）。
pub const USER_STACK_VADDR: u64 = USER_CODE_VADDR + 0x20_0000;

// boot.asm 导出的内核 PDPT（恒等映射 0~1GB）。
extern "C" {
    static pdpt: u8;
}

/// 用户态测试代码（机器码）：write(1, msg, 21) 然后 exit(0)。
///
/// ```asm
/// mov rax, 1            ; SYS_WRITE
/// mov rdi, 1            ; fd = 1
/// mov rsi, 0x80_0000_0100  ; msg 地址（代码页 +0x100）
/// mov rdx, 21           ; len
/// syscall
/// mov rax, 60           ; SYS_EXIT
/// xor rdi, rdi
/// syscall
/// ```
const USER_CODE: [u8; 43] = [
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
    0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00, // mov rdi, 1
    0x48, 0xBE, 0x00, 0x01, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, // mov rsi, 0x80_0000_0100
    0xBA, 0x15, 0x00, 0x00, 0x00, // mov edx, 21
    0x0F, 0x05, // syscall (write)
    0x48, 0xC7, 0xC0, 0x3C, 0x00, 0x00, 0x00, // mov rax, 60
    0x48, 0x31, 0xFF, // xor rdi, rdi
    0x0F, 0x05, // syscall (exit)
];

const USER_MSG: &[u8] = b"hello from userspace\n";

/// 用户进程页表。
pub struct UserPageTable {
    pub pml4: usize,
    pub code_phys: usize,
    pub stack_phys: usize,
}

/// 构建用户进程页表：分配代码页（2MB）+ 栈页（2MB）+ 三级页表。
pub fn build() -> UserPageTable {
    // 2MB 对齐的物理页（order 9）。
    let code_phys = mm::alloc_pages(9);
    let stack_phys = mm::alloc_pages(9);
    let pml4 = mm::alloc_pages(0);
    let user_pdpt = mm::alloc_pages(0);
    let user_pd = mm::alloc_pages(0);
    assert!(code_phys != 0 && stack_phys != 0, "usermode: phys alloc failed");

    // SAFETY: 分配的物理页，恒等映射下直接访问。
    unsafe {
        // 清零页表页
        core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
        core::ptr::write_bytes(user_pdpt as *mut u8, 0, 4096);
        core::ptr::write_bytes(user_pd as *mut u8, 0, 4096);

        // PML4[0] = 内核 PDPT（恒等映射 0~1GB，supervisor）
        let kpdpt = core::ptr::addr_of!(pdpt) as u64;
        *(pml4 as *mut u64) = kpdpt | P_PRESENT | P_WRITABLE;
        // PML4[1] = 用户 PDPT（512GB 起，USER）
        *((pml4 as *mut u64).add(1)) = user_pdpt as u64 | P_PRESENT | P_WRITABLE | P_USER;
        // 用户 PDPT[0] = 用户 PD
        *(user_pdpt as *mut u64) = user_pd as u64 | P_PRESENT | P_WRITABLE | P_USER;
        // 用户 PD[0] = 代码（2MB 大页），PD[1] = 栈（2MB 大页）
        *(user_pd as *mut u64) = code_phys as u64 | P_PRESENT | P_WRITABLE | P_USER | P_HUGE;
        *((user_pd as *mut u64).add(1)) = stack_phys as u64 | P_PRESENT | P_WRITABLE | P_USER | P_HUGE;

        // 写用户态代码 + msg 到代码页
        core::ptr::copy_nonoverlapping(USER_CODE.as_ptr(), code_phys as *mut u8, USER_CODE.len());
        core::ptr::copy_nonoverlapping(USER_MSG.as_ptr(), (code_phys + 0x100) as *mut u8, USER_MSG.len());
    }

    UserPageTable { pml4, code_phys, stack_phys }
}

/// 切换到用户页表并 `iretq` 进入 ring3（不返回）。
///
/// # Safety
/// 仅启动阶段调用一次；此后 CPU 处于用户态，直到 syscall exit 停机。
pub fn enter_user(pt: &UserPageTable) -> ! {
    let stack_top = USER_STACK_VADDR + 0x10_0000; // 栈顶（2MB 栈中部）
    // SAFETY: 切换 CR3 + iretq 为特权指令。
    unsafe {
        // 切换到用户页表
        core::arch::asm!("mov cr3, {0}", in(reg) pt.pml4, options(nostack, nomem));
        // 设置数据段寄存器为用户数据段，构造 iretq 帧跳 ring3
        core::arch::asm!(
            "mov ds, {ds}",
            "mov es, {ds}",
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ds = in(reg) USER_DATA_SEL as u64,
            ss = in(reg) USER_DATA_SEL as u64,
            rsp = in(reg) stack_top,
            rflags = in(reg) 0x2u64, // IF=0：关中断，避免 PIT tick 抢占用户态（syscall 不受 IF 影响）
            cs = in(reg) USER_CODE_SEL as u64,
            rip = in(reg) USER_CODE_VADDR,
            options(noreturn)
        );
    }
}
