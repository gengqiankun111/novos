//! 8250 UART 串口驱动（DESIGN.md §1.3 Phase 2 / §10.1 日志出口）。
//!
//! 自包含实现（不依赖外部 crate）：COM1 = 0x3F8，115200-8N1。
//! M0 无堆、无 `alloc`：`write_fmt` 不分配，满足 Phase 1 "无堆" 约束。

use crate::port::Port;
use core::fmt::Write;
use spin::Mutex;

/// COM1 寄存器偏移（相对 0x3F8）。
const THR: u16 = 0; // 发送保持寄存器（写）
const DLL: u16 = 0; // 分频低字节（DLAB=1 时）
const IER: u16 = 1; // 中断使能
const DLM: u16 = 1; // 分频高字节（DLAB=1 时）
const FCR: u16 = 2; // FIFO 控制
const LCR: u16 = 3; // 线控制
const MCR: u16 = 4; // 调制解调器控制
const LSR: u16 = 5; // 线状态

const LSR_THR_EMPTY: u8 = 0x20; // 发送保持寄存器空

static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort);

/// 8250 串口（最小驱动：init + 单字节发送）。
struct SerialPort;

impl SerialPort {
    fn init(&mut self) {
        // 关闭中断
        Self::write_reg(IER, 0x00);
        // DLAB=1，设置分频 1 → 115200 baud
        Self::write_reg(LCR, 0x80);
        Self::write_reg(DLL, 0x01);
        Self::write_reg(DLM, 0x00);
        // 8 数据位、无校验、1 停止位
        Self::write_reg(LCR, 0x03);
        // 使能 FIFO（清缓冲、14 字节阈值）
        Self::write_reg(FCR, 0xC7);
        // DTR+RTS（置 0x0B 还会开 OUT2/中断，保持 0x03 即可）
        Self::write_reg(MCR, 0x03);
    }

    fn write_byte(&mut self, byte: u8) {
        // 等待 THR 空
        while Self::read_reg(LSR) & LSR_THR_EMPTY == 0 {}
        Self::write_reg(THR, byte);
    }

    /// COM1 基址。
    const fn base_addr() -> u16 {
        0x3F8
    }

    fn write_reg(reg: u16, val: u8) {
        // SAFETY: 0x3F8+reg 为 COM1 寄存器偏移，端口可写。
        let mut p = unsafe { Port::new(Self::base_addr() + reg) };
        unsafe { p.write(val) };
    }

    fn read_reg(reg: u16) -> u8 {
        // SAFETY: 0x3F8+reg 为 COM1 寄存器偏移，端口可读。
        let mut p = unsafe { Port::new(Self::base_addr() + reg) };
        unsafe { p.read() }
    }
}

/// 初始化串口。
pub fn init() {
    SERIAL.lock().init();
}

/// 带锁输出格式化字符串（`fmt::Write` 对串口永不出错，忽略 Result）。
pub fn print_fmt(args: core::fmt::Arguments) {
    let mut serial = SERIAL.lock();
    let _ = write_all(&mut serial, args);
}

/// 输出一行。
pub fn println_fmt(args: core::fmt::Arguments) {
    print_fmt(args);
    print_fmt(format_args!("\n"));
}

/// 直接向 `SerialPort` 写格式化内容，并镜像到 VGA 文本屏（QEMU screendump 截屏依赖）。
///
/// 用"双写 writer"而非 `format!`，保持零堆分配（打印可能发生在持分配器锁期间）。
fn write_all(serial: &mut SerialPort, args: core::fmt::Arguments) -> core::fmt::Result {
    struct Dual<'a> {
        serial: &'a mut SerialPort,
    }
    impl Write for Dual<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for b in s.bytes() {
                self.serial.write_byte(b);
            }
            // 镜像到 VGA（无锁：M1 单核，原子光标足够）。
            crate::vga::write_str(s);
            Ok(())
        }
    }
    Dual { serial }.write_fmt(args)
}

/// panic 专用输出：绕过锁直接写端口，避免"持锁中 panic → 自旋死锁"。
pub fn panic_write(args: core::fmt::Arguments) {
    struct RawWriter {
        base: u16,
    }
    impl Write for RawWriter {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for b in s.bytes() {
                // 等待 THR 空
                let mut lsr = unsafe { Port::<u8>::new(self.base + LSR) };
                while unsafe { lsr.read() } & LSR_THR_EMPTY == 0 {}
                let mut thr = unsafe { Port::<u8>::new(self.base + THR) };
                unsafe { thr.write(b) };
            }
            // 镜像到 VGA（panic 也可见，便于 screendump 捕获）。
            crate::vga::write_str(s);
            Ok(())
        }
    }

    let _ = RawWriter { base: 0x3F8 }.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::serial::print_fmt(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::serial::println_fmt(format_args!("")));
    ($($arg:tt)*) => ($crate::serial::println_fmt(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! panic_println {
    ($($arg:tt)*) => ($crate::serial::panic_write(format_args!($($arg)*)));
}
