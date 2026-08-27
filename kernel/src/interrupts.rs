//! IDT / PIC / 异常处理（DESIGN.md §1.4）。
//!
//! M0 阶段：IDT 填充 0–31 异常向量（指向 boot.asm 的 16 字节 stub 槽），
//! PIC 重映射到 0x20–0x2F 并屏蔽全部外设中断（定时器留到 M2）。
//! 异常统一走 `rust_exception_handler`：打印 + 停机（M0 无用户态，异常即致命）。

use crate::port::Port;
use crate::serial::panic_write;
use core::arch::asm;

/// IDT 条目（对应 DESIGN.md §1.4 的 `IdtEntry` 结构）。
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,   // handler 低 16 位
    selector: u16,     // 段选择子（内核代码段，boot GDT 的 0x08）
    ist: u8,           // IST 索引（M0 全部为 0；IST 栈 M2 引入）
    flags: u8,         // P | DPL | Type
    offset_mid: u16,   // handler 中 16 位
    offset_high: u32,  // handler 高 32 位
    reserved: u32,
}

impl IdtEntry {
    const fn null() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// 构造指向 `handler_addr` 的中断门（DPL=0，ist=0）。
    const fn gate(handler_addr: u64, selector: u16) -> Self {
        Self {
            offset_low: handler_addr as u16,
            selector,
            ist: 0,
            flags: 0x8E,
            offset_mid: (handler_addr >> 16) as u16,
            offset_high: (handler_addr >> 32) as u32,
            reserved: 0,
        }
    }
}

/// IDT（256 项）。
pub struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    const fn new() -> Self {
        Self {
            entries: [IdtEntry::null(); 256],
        }
    }

    fn set_gate(&mut self, index: u8, handler_addr: u64) {
        self.entries[index as usize] = IdtEntry::gate(handler_addr, KERNEL_CODE_SELECTOR);
    }

    /// 用 `lidt` 加载。
    fn load(&'static self) {
        let limit = (core::mem::size_of::<Self>() - 1) as u16;
        let base = self.entries.as_ptr() as u64;
        // 64 位模式下 lidt 读 10 字节（limit16 + base64），用全 64 位地址操作数。
        unsafe {
            asm!(
                "lidt [{0}]",
                in(reg) &IdtDescriptor { limit, base } as *const IdtDescriptor,
                options(nostack, preserves_flags)
            );
        }
    }
}

/// IDTR 描述符（10 字节，packed）。
#[repr(C, packed)]
struct IdtDescriptor {
    limit: u16,
    base: u64,
}

/// 内核代码段选择子（boot.asm 中 gdt64 的 code64）。
const KERNEL_CODE_SELECTOR: u16 = 0x08;

/// 中断向量：32–47 为 PIC 外设中断（重映射后），对应 DESIGN §1.4。
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

// boot.asm 导出的异常 stub 表基址（每个 stub 恰好 16 字节）。
#[cfg(not(test))]
extern "C" {
    static stub_base: u8;
    static irq_stub_base: u8;
}
// host 单测桩：boot.asm 不参与测试链接，提供同名静态占位。
#[cfg(test)]
pub static stub_base: u8 = 0;
#[cfg(test)]
pub static irq_stub_base: u8 = 0;

/// 异常保存的寄存器帧（对应 boot.asm `exception_common` 的压栈顺序）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExceptionFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vec: u64,
    pub err: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// 全局 IDT（静态分配，M0 无堆）。
static mut IDT: Idt = Idt::new();

/// 初始化 IDT + PIC，并开中断。
pub fn init() {
    let stub_base_addr = unsafe { &stub_base as *const u8 as u64 };

    // 填充 0–31 CPU 异常（每 16 字节一个 stub）
    // SAFETY: 单核启动阶段独占访问静态 IDT。
    let idt_ptr = core::ptr::addr_of_mut!(IDT);
    unsafe {
        for i in 0..32u8 {
            (*idt_ptr).set_gate(i, stub_base_addr + u64::from(i) * 16);
        }
        // 填充 32–47 外设中断（PIC 重映射后）
        let irq_stub_base_addr = &irq_stub_base as *const u8 as u64;
        for i in 0..16u8 {
            (*idt_ptr).set_gate(PIC_1_OFFSET + i, irq_stub_base_addr + u64::from(i) * 16);
        }
    }

    // SAFETY: 单核启动阶段独占访问，lidt 立即生效。
    unsafe {
        (&*core::ptr::addr_of!(IDT)).load();
    }

    // PIC 重映射：PIC1 → 0x20-0x27，PIC2 → 0x28-0x2F（避开 CPU 异常 0-31）。
    // SAFETY: 8259A 编程序列（ICW1-4 + OCW1），端口 0x20/0x21/0xA0/0xA1 为标准控制器端口。
    unsafe {
        // ICW1: 级联 + 需要 ICW4
        let mut p1 = Port::<u8>::new(0x20);
        let mut p2 = Port::<u8>::new(0xA0);
        p1.write(0x11);
        p2.write(0x11);
        // ICW2: 中断向量偏移
        let mut p1d = Port::<u8>::new(0x21);
        let mut p2d = Port::<u8>::new(0xA1);
        p1d.write(PIC_1_OFFSET);
        p2d.write(PIC_2_OFFSET);
        // ICW3: 级联配置
        p1d.write(0x04); // PIC1 从片接在 IRQ2
        p2d.write(0x02); // PIC2 级联 ID=2
        // ICW4: 8086 模式
        p1d.write(0x01);
        p2d.write(0x01);
        // OCW1: 屏蔽除 IRQ0（PIT 定时器）外全部外设中断；IRQ1 键盘等 M2 后按需开
        p1d.write(0xFE);
        p2d.write(0xFF);
    }

    // 等 IO 完成（PIC 初始化后需要少量 IO 周期）。
    // SAFETY: 0x80 为传统 IO 延迟端口。
    unsafe {
        let mut port = Port::<u8>::new(0x80);
        port.write(0);
    }

    // 开中断（当前全部 IRQ 被屏蔽，仅 CPU 异常/NMI 可达）。
    // SAFETY: sti 为特权指令。
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// 异常统一入口：打印寄存器快照 + 停机（M0 异常即致命，DESIGN §8.2）。
///
/// # Safety
/// 由 boot.asm 的 `exception_common` 以有效帧指针调用。
#[no_mangle]
pub unsafe extern "C" fn rust_exception_handler(frame: *const ExceptionFrame) -> ! {
    let f = unsafe { &*frame };
    panic_write(format_args!("EXCEPTION: vector={} err={:#x}\n", f.vec, f.err));
    // 读取 CR2（页错误地址）
    let cr2: u64;
    // SAFETY: mov %cr2 为只读特权指令。
    unsafe { asm!("movq %cr2, {0}", out(reg) cr2, options(nomem, nostack, att_syntax)) };
    panic_write(format_args!("  cr2={:#x}\n", cr2));
    panic_write(format_args!("  rip={:#x} cs={:#x} rflags={:#x}\n", f.rip, f.cs, f.rflags));
    panic_write(format_args!("  rsp={:#x} ss={:#x}\n", f.rsp, f.ss));
    panic_write(format_args!("  rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x}\n", f.rax, f.rbx, f.rcx, f.rdx));
    panic_write(format_args!("  rdi={:#x} rsi={:#x} rbp={:#x}\n", f.rdi, f.rsi, f.rbp));
    loop {
        // SAFETY: hlt 为特权指令。
        unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)); }
    }
}

/// 供启动日志确认 IDT 已加载。
pub fn info() -> &'static str {
    "idt(0-47)/pic ready"
}

/// 向 PIC 发送 EOI（中断服务结束）。
pub fn pic_eoi(irq: u8) {
    // SAFETY: 8259A 标准 EOI 命令端口。
    unsafe {
        let mut p1 = Port::<u8>::new(0x20);
        p1.write(0x20);
        if irq >= 8 {
            let mut p2 = Port::<u8>::new(0xA0);
            p2.write(0x20);
        }
    }
}

/// 外设中断统一入口（由 boot.asm `irq_common` 调用）。
///
/// 发送 EOI 后委托调度器处理；返回目标任务应恢复的 `ExceptionFrame` 指针
/// （未切换时返回原帧）。M2 仅开 IRQ0（PIT），其余 IRQ 保持屏蔽。
///
/// # Safety
/// 由 boot.asm 以有效帧指针调用；此时中断处于关闭状态（中断门）。
#[no_mangle]
pub unsafe extern "C" fn rust_irq_handler(frame: *mut ExceptionFrame) -> *mut ExceptionFrame {
    // SAFETY: frame 由 irq_common 构造，vec 字段有效。
    let vec = unsafe { (*frame).vec };
    pic_eoi((vec - u64::from(PIC_1_OFFSET)) as u8);
    // SAFETY: 单核、IF 关闭，调度器无重入。
    unsafe { crate::task::on_timer_tick(frame) }
}
