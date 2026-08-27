//! x86-64 端口 I/O 最小封装（in/out 指令）。
//!
//! 仅 panic 路径与启动早期使用；常规设备（串口/PIC）由各自驱动内部完成 I/O。

use core::marker::PhantomData;

/// 端口抽象（`U` 为数据宽度标记）。
pub struct Port<U> {
    addr: u16,
    _marker: PhantomData<U>,
}

impl Port<u8> {
    /// 构造端口。
    ///
    /// # Safety
    /// 调用者必须保证 `addr` 是合法的 I/O 端口且具备访问权限。
    pub const unsafe fn new(addr: u16) -> Self {
        Self {
            addr,
            _marker: PhantomData,
        }
    }

    /// 输出一个字节。
    ///
    /// # Safety
    /// 端口必须已由调用方确认可写。
    pub unsafe fn write(&mut self, val: u8) {
        core::arch::asm!("out dx, al", in("dx") self.addr, in("al") val, options(nomem, nostack, preserves_flags));
    }

    /// 输入一个字节。
    ///
    /// # Safety
    /// 端口必须已由调用方确认可读。
    pub unsafe fn read(&mut self) -> u8 {
        let val: u8;
        core::arch::asm!("in al, dx", out("al") val, in("dx") self.addr, options(nomem, nostack, preserves_flags));
        val
    }
}
