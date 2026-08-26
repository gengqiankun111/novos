//! M4 切片1：VFS 核心 + ramfs 根文件系统 + 路径解析。
//!
//! 结构（DESIGN §3.6，务实最小版：独立 dcache 层留 M4-切片3）：
//! - `Inode`：mode / 内容 / 目录子项（Arc 共享，目录即子项表）；
//! - 根 "/" = ramfs 目录；仅支持绝对路径；
//! - `File` 扩展：Reg 文件（inode + 读写偏移）、Uart（fd 0/1/2）。
//! - fd 表沿用 M3 收尾（Vec<Option<Arc<File>>> + 低 64 位空闲位图）。

use crate::serial;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

// ---- 文件类型位（Linux S_IF*）----
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;

// ---- open flags（Linux O_*）----
pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_RDWR: u64 = 2;
pub const O_CREAT: u64 = 0o100;
pub const O_TRUNC: u64 = 0o1000;
pub const O_APPEND: u64 = 0o2000;

/// 目录项：名字 + inode（M4-切片3 抽出独立 dcache 前暂存于父目录）。
pub struct Dentry {
    pub name: String,
    pub inode: Arc<Inode>,
}

/// inode：ramfs 文件/目录统一结构。
pub struct Inode {
    /// 类型位（S_IFREG/S_IFDIR）| 权限（M3 简化全 0644/0755）。
    pub mode: u32,
    /// 链接计数（M3 简化：目录 2，文件 1）。
    pub nlink: u32,
    /// 文件内容（size 即 data.len()）。
    pub data: Mutex<Vec<u8>>,
    /// 目录子项。
    pub children: Mutex<Vec<Dentry>>,
}

impl Inode {
    fn new(mode: u32, nlink: u32) -> Arc<Inode> {
        Arc::new(Inode {
            mode,
            nlink,
            data: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
        })
    }

    /// 常规文件 inode。
    fn file() -> Arc<Inode> {
        Self::new(S_IFREG | 0o644, 1)
    }

    /// 目录 inode。
    fn dir() -> Arc<Inode> {
        Self::new(S_IFDIR | 0o755, 2)
    }

    pub fn is_dir(&self) -> bool {
        self.mode & S_IFDIR != 0
    }

    pub fn is_file(&self) -> bool {
        self.mode & S_IFREG != 0
    }

    pub fn size(&self) -> usize {
        self.data.lock().len()
    }

    /// 在当前目录 inode 中按名字查找子项。
    fn lookup(&self, name: &str) -> Option<Arc<Inode>> {
        self.children
            .lock()
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.inode.clone())
    }

    /// 添加子项。
    fn insert_child(&self, name: &str, ino: Arc<Inode>) {
        self.children.lock().push(Dentry {
            name: String::from(name),
            inode: ino,
        });
    }
}

/// 根文件系统（ramfs，挂载于 "/"）。预置 /etc 目录（M3: /etc/motd 登录提示）。
pub static ROOT: spin::Lazy<Arc<Inode>> = spin::Lazy::new(|| {
    let root = Inode::dir();
    let etc = Inode::dir();
    root.insert_child("etc", etc);
    root
});

/// 文件句柄（fd 表元素）。
pub enum File {
    /// /dev/uart：read = 非阻塞串口读，write = 串口输出（镜像 VGA）。
    Uart,
    /// 常规文件（ramfs）：inode + 当前读写偏移。
    Reg {
        inode: Arc<Inode>,
        offset: Mutex<u64>,
    },
}

impl File {
    /// 读：返回写入 buf 的字节数（非阻塞；文件 EOF 返回 0）。
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
            File::Reg { inode, offset } => {
                let data = inode.data.lock();
                let mut off = offset.lock();
                let start = *off as usize;
                let n = core::cmp::min(buf.len(), data.len().saturating_sub(start));
                buf[..n].copy_from_slice(&data[start..start + n]);
                *off = (start + n) as u64;
                n
            }
        }
    }

    /// 写：返回已写字节数（文件在偏移处写入并扩展）。
    pub fn write(&self, data: &[u8]) -> usize {
        match self {
            File::Uart => {
                let s = core::str::from_utf8(data).unwrap_or("<non-utf8>");
                crate::print!("{}", s);
                data.len()
            }
            File::Reg { inode, offset } => {
                let mut off = offset.lock();
                let start = *off as usize;
                let mut d = inode.data.lock();
                let end = start + data.len();
                if d.len() < end {
                    d.resize(end, 0);
                }
                d[start..end].copy_from_slice(data);
                *off = end as u64;
                data.len()
            }
        }
    }
}

/// 路径解析：从根逐组件查找（仅绝对路径；"." 跳过，".." 简化不支持）。
/// `create_last`：末组件不存在时创建文件（仅当父目录存在）。
fn resolve(path: &str, create_last: bool) -> Result<Arc<Inode>, ()> {
    if !path.starts_with('/') {
        return Err(());
    }
    let mut cur = ROOT.clone();
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    for (i, comp) in comps.iter().enumerate() {
        let last = i == comps.len() - 1;
        match cur.lookup(comp) {
            Some(ino) => cur = ino,
            None => {
                if last && create_last {
                    let ino = Inode::file();
                    cur.insert_child(comp, ino.clone());
                    cur = ino;
                } else {
                    return Err(());
                }
            }
        }
    }
    Ok(cur)
}

/// 打开路径。M3 兼容 "/dev/uart"；其余走 ramfs（O_CREAT 创建文件）。
/// 成功返回 File，失败返回负 errno。
pub fn open_path(name: &str, flags: u64) -> Result<Arc<File>, i64> {
    if name == "/dev/uart" {
        return Ok(Arc::new(File::Uart));
    }
    let create = flags & O_CREAT != 0;
    let inode = resolve(name, create).map_err(|_| -2i64)?; // ENOENT
    if inode.is_dir() {
        return Err(-21i64); // EISDIR（目录打开留 M4-切片2）
    }
    if flags & O_TRUNC != 0 {
        inode.data.lock().clear();
    }
    let offset = if flags & O_APPEND != 0 {
        inode.data.lock().len() as u64
    } else {
        0
    };
    Ok(Arc::new(File::Reg {
        inode,
        offset: Mutex::new(offset),
    }))
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

/// 全局 fd 表（spin::Lazy 惰性初始化；单进程 init，进程模型后随任务迁移）。
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
