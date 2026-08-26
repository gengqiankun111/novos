//! M3 收尾：文件描述符表 + /dev/uart 设备文件（M4 VFS 前置）。
//!
//! 设计（DESIGN §3.6 / 数据结构评审定案）：
//! - fd 表 = `Vec<Option<Arc<File>>>` + 低 64 位空闲位图（稠密整数数组 O(1) 分配，
//!   非 BTreeMap）；
//! - `File` 为枚举抽象（M3 仅 Uart；M4 扩展 inode 文件/管道/目录）；
//! - 0/1/2 = stdin/stdout/stderr 均指向 /dev/uart。

use crate::serial;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// 文件类型（M3 仅 /dev/uart）。
pub enum File {
    /// UART 设备：read = 非阻塞串口读，write = 串口输出（镜像 VGA）。
    Uart,
}

impl File {
    /// 读：返回写入 buf 的字节数（非阻塞，无数据返回 0）。
    pub fn read(&self, buf: &mut [u8]) -> usize {
        match self {
            File::Uart => {
                if let Some(b) = serial::read_byte() {
                    if !buf.is_empty() {
                        buf[0] = b;
                        1
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
        }
    }

    /// 写：返回已写字节数（UART 无缓冲，全量写出）。
    pub fn write(&self, data: &[u8]) -> usize {
        match self {
            File::Uart => {
                let s = core::str::from_utf8(data).unwrap_or("<non-utf8>");
                crate::print!("{}", s);
                data.len()
            }
        }
    }
}

/// 文件描述符表。
pub struct FdTable {
    slots: Vec<Option<Arc<File>>>,
    /// 低 64 位 fd 的空闲位图（1 = 空闲；fd 0-2 初始化占用）。
    free_bits: u64,
}

impl FdTable {
    /// 创建：0/1/2 = stdin/stdout/stderr → /dev/uart。
    pub fn new() -> Self {
        let mut slots = Vec::new();
        for _ in 0..3 {
            slots.push(Some(Arc::new(File::Uart)));
        }
        FdTable {
            slots,
            free_bits: !0u64 << 3, // bit0-2 已占用
        }
    }

    /// 分配新 fd（位图优先 O(1)，位图外线性找空槽/追加）。
    pub fn alloc(&mut self, f: Arc<File>) -> usize {
        if self.free_bits != 0 {
            let bit = self.free_bits.trailing_zeros() as usize;
            self.free_bits &= !(1u64 << bit);
            while self.slots.len() <= bit {
                self.slots.push(None);
            }
            self.slots[bit] = Some(f);
            return bit;
        }
        for (i, s) in self.slots.iter_mut().enumerate() {
            if s.is_none() {
                *s = Some(f);
                return i;
            }
        }
        let fd = self.slots.len();
        self.slots.push(Some(f));
        fd
    }

    /// 关闭 fd；成功返回 true。
    pub fn close(&mut self, fd: usize) -> bool {
        if fd < self.slots.len() && self.slots[fd].take().is_some() {
            if fd < 64 {
                self.free_bits |= 1u64 << fd;
            }
            return true;
        }
        false
    }

    /// 取 fd 对应文件（clone Arc 增引用）。
    pub fn get(&self, fd: usize) -> Option<Arc<File>> {
        self.slots.get(fd).and_then(|s| s.clone())
    }
}

/// 全局 fd 表（spin::Lazy 惰性初始化；单进程 init，M4 进程模型后随任务迁移）。
static FD_TABLE: spin::Lazy<Mutex<FdTable>> = spin::Lazy::new(|| Mutex::new(FdTable::new()));

/// 分配 fd。
pub fn fd_alloc(f: Arc<File>) -> usize {
    FD_TABLE.lock().alloc(f)
}

/// 关闭 fd。
pub fn fd_close(fd: usize) -> bool {
    FD_TABLE.lock().close(fd)
}

/// 取 fd 对应文件。
pub fn fd_get(fd: usize) -> Option<Arc<File>> {
    FD_TABLE.lock().get(fd)
}

/// 打开路径（M3 仅支持 "/dev/uart"）；失败返回 None。
pub fn open_path(name: &str) -> Option<Arc<File>> {
    match name {
        "/dev/uart" => Some(Arc::new(File::Uart)),
        _ => None,
    }
}
