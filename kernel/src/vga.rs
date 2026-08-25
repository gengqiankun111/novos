//! x86 文本模式 VGA（0xB8000，80x25）——让 QEMU `screendump` 能截到内核输出。
//!
//! 与串口输出镜像：serial.rs 每条输出同时写 VGA（见 serial.rs `write_all`/`panic_write`）。

use core::sync::atomic::{AtomicUsize, Ordering};

/// 文本模式帧缓冲基址。
const VGA_BUF: *mut u8 = 0xB8000 as *mut u8;
const COLS: usize = 80;
const ROWS: usize = 25;
const COLOR_LIGHT_GRAY_ON_BLACK: u8 = 0x07;

static ROW: AtomicUsize = AtomicUsize::new(0);
static COL: AtomicUsize = AtomicUsize::new(0);

/// 清屏并归位光标。
pub fn init() {
    // SAFETY: 0xB8000 是 x86 文本模式帧缓冲，25*80*2 字节可写。
    unsafe {
        core::ptr::write_bytes(VGA_BUF, 0, COLS * ROWS * 2);
    }
    ROW.store(0, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
}

/// 写一段字符串到 VGA（自动换行/滚动）。
pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            newline();
        } else {
            put(b);
        }
    }
}

fn put(b: u8) {
    let mut col = COL.load(Ordering::Relaxed);
    let row = ROW.load(Ordering::Relaxed);
    if col >= COLS {
        newline();
        col = COL.load(Ordering::Relaxed);
    }
    let idx = (row * COLS + col) * 2;
    // SAFETY: idx < 25*80*2，且 row/col 维护在边界内。
    unsafe {
        core::ptr::write_volatile(VGA_BUF.add(idx), b);
        core::ptr::write_volatile(VGA_BUF.add(idx + 1), COLOR_LIGHT_GRAY_ON_BLACK);
    }
    COL.store(col + 1, Ordering::Relaxed);
}

fn newline() {
    let row = ROW.load(Ordering::Relaxed);
    if row + 1 >= ROWS {
        scroll();
    } else {
        ROW.store(row + 1, Ordering::Relaxed);
    }
    COL.store(0, Ordering::Relaxed);
}

/// 上滚一行：第 1..24 行整体上移，末行清空。
fn scroll() {
    // SAFETY: 24*80*2 字节在缓冲内。
    unsafe {
        core::ptr::copy(VGA_BUF.add(COLS * 2), VGA_BUF, (ROWS - 1) * COLS * 2);
        core::ptr::write_bytes(VGA_BUF.add((ROWS - 1) * COLS * 2), 0, COLS * 2);
    }
}
