//! M2：PIT 8254 定时器（IRQ0 → 向量 32，100Hz 时基）。
//!
//! 为 CFS 调度器提供 tick 时钟源（M2 最小实现；时钟源抽象/MONOTONIC 见 M9）。
//! PIT 通道 0：模式 3（方波），先写低字节再写高字节。

use crate::port::Port;

/// PIT 输入时钟（1.19318 MHz）。
const PIT_FREQ: u64 = 1_193_182;
/// 调度 tick 频率（100Hz → 10ms 时基）。
pub const HZ: u64 = 100;

const PIT_CH0: u16 = 0x40; // 通道 0 数据端口
const PIT_CMD: u16 = 0x43; // 模式/命令寄存器

/// 初始化 PIT：100Hz 方波。之后需由 interrupts::init 打开 IRQ0（OCW1=0xFE）。
pub fn init() {
    let divisor = (PIT_FREQ / HZ) as u16; // 11932 → ≈100Hz
    // SAFETY: 标准 8254 编程序列，端口 0x43/0x40 可写。
    unsafe {
        let mut cmd = Port::<u8>::new(PIT_CMD);
        cmd.write(0x36); // 通道 0 | 先低后高 | 模式 3 | 二进制
        let mut ch0 = Port::<u8>::new(PIT_CH0);
        ch0.write((divisor & 0xFF) as u8);
        ch0.write((divisor >> 8) as u8);
    }
}
