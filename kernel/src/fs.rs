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
    /// 目录（ramfs）：供 getdents64 枚举（pos = 已返回项数游标）。
    Dir {
        inode: Arc<Inode>,
        pos: Mutex<usize>,
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
            File::Dir { .. } => 0,
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
            File::Dir { .. } => 0,
        }
    }
}

/// 挂载点：挂载路径 → 独立文件系统根（M4-切片4：tmpfs 简化实现）。
/// 挂载类型。
pub enum MountKind {
    /// tmpfs/ramfs：独立树。
    Tmpfs(Arc<Inode>),
    /// overlay（M7-切片1）：lower（只读）+ upper（可写）合并视图。
    Overlay {
        lower: Arc<Inode>,
        upper: Arc<Inode>,
    },
    /// proc（M8-切片4）：pid ns 视图——按当前任务 ns 动态列任务。
    Proc(Arc<Inode>),
}

pub struct Mount {
    pub path: String,
    pub kind: MountKind,
}

/// 挂载表。
static MOUNTS: spin::Lazy<Mutex<Vec<Mount>>> = spin::Lazy::new(|| Mutex::new(Vec::new()));

/// 当前活跃 overlay（{ 挂载路径, upper, lower }）——M7-切片1 单 overlay 简化。
static ACTIVE_OVERLAY: spin::Lazy<Mutex<Option<(String, Arc<Inode>, Arc<Inode>)>>> =
    spin::Lazy::new(|| Mutex::new(None));

/// 查找挂载项（精确路径匹配）。
fn mount_entry(path: &str) -> Option<Mount> {
    // 返回克隆（避免锁嵌套；demo 规模足够）。
    MOUNTS.lock().iter().find(|m| m.path == path).map(|m| {
        let kind = match &m.kind {
            MountKind::Tmpfs(r) => MountKind::Tmpfs(r.clone()),
            MountKind::Overlay { lower, upper } => MountKind::Overlay {
                lower: lower.clone(),
                upper: upper.clone(),
            },
            MountKind::Proc(r) => MountKind::Proc(r.clone()),
        };
        Mount {
            path: m.path.clone(),
            kind,
        }
    })
}

/// 查找挂载点根 inode（精确路径匹配；overlay 返回 merged 上层）。
pub fn mount_lookup(path: &str) -> Option<Arc<Inode>> {
    mount_entry(path).map(|m| match m.kind {
        MountKind::Tmpfs(r) => r,
        MountKind::Overlay { upper, .. } => upper,
        MountKind::Proc(r) => r,
    })
}

/// 挂载新 tmpfs 根到 target 目录。
pub fn mount_fs(target: &str) -> Result<(), i64> {
    if target == "/" || target.is_empty() {
        return Err(-16i64); // EBUSY：根目录不可挂
    }
    let tgt = resolve(target, false).map_err(|_| -2i64)?; // ENOENT
    if !tgt.is_dir() {
        return Err(-20i64); // ENOTDIR
    }
    if mount_entry(target).is_some() {
        return Err(-16i64); // EBUSY：已挂载
    }
    MOUNTS.lock().push(Mount {
        path: String::from(target),
        kind: MountKind::Tmpfs(Inode::dir()),
    });
    Ok(())
}

/// /proc 挂载根（M8-切片4：pid ns 视图；按指针相等判定）。
pub static PROC_ROOT: spin::Lazy<Arc<Inode>> = spin::Lazy::new(|| Inode::dir());

/// 挂载 proc 到 target 目录（pid ns 视图）。
pub fn mount_fs_proc(target: &str) -> Result<(), i64> {
    if target == "/" || target.is_empty() {
        return Err(-16i64);
    }
    let tgt = resolve(target, false).map_err(|_| -2i64)?;
    if !tgt.is_dir() {
        return Err(-20i64);
    }
    if mount_entry(target).is_some() {
        return Err(-16i64);
    }
    MOUNTS.lock().push(Mount {
        path: String::from(target),
        kind: MountKind::Proc(PROC_ROOT.clone()),
    });
    Ok(())
}

// ---- M9-切片1：/proc/health + /proc/cpuinfo（健康指标 + 多核报告）----

/// 打开 fd 数（/proc/health 用）。
pub fn open_fd_count() -> usize {
    FD_TABLE.lock().slots.iter().filter(|s| s.is_some()).count()
}

/// /proc/health：JSON 健康指标（内存 used/free、fd 数、容器数、CPU 负载）。
pub fn health_inode() -> Arc<Inode> {
    let ino = Inode::file();
    let st = crate::mm::mem_stats();
    let total = 64 * 1024 * 1024usize;
    let used = st.kernel_used_bytes;
    let fds = open_fd_count();
    let containers = crate::task::container_count();
    // CPU 负载近似：所有任务累计运行 tick / 总 tick（忙占比）
    let busy = crate::task::busy_ticks();
    let total_ticks = crate::task::ticks().max(1);
    let load = (busy as u64 * 100 / total_ticks) as u64;
    let body = alloc::format!(
        "{{\"mem_used\":{},\"mem_free\":{},\"fds\":{},\"containers\":{},\"cpu_load\":{}}}\n",
        used,
        total.saturating_sub(used),
        fds,
        containers,
        load
    );
    *ino.data.lock() = body.into_bytes();
    ino
}

/// /proc/cpuinfo：多核报告（SMP 前 online 只显示 1，避免误判，DESIGN §21.5）。
pub fn cpuinfo_inode() -> Arc<Inode> {
    let ino = Inode::file();
    let body = alloc::format!(
        "processor\t: 0\nvendor_id\t: Novos\nmodel name\t: Novos-OS virtual CPU\ncpu MHz\t\t: 100\ncores\t\t: 1\nphysical id\t: 0\ncpu cores\t: 1\nonline\t\t: 1\n"
    );
    *ino.data.lock() = body.into_bytes();
    ino
}

/// 判断 proc 子文件（health/cpuinfo）——resolve 用。
fn proc_special_file(name: &str) -> Option<Arc<Inode>> {
    match name {
        "health" => Some(health_inode()),
        "cpuinfo" => Some(cpuinfo_inode()),
        _ => None,
    }
}

/// /proc/self（M13-01：当前任务视图，指针相等判定）。
pub static SELF_DIR: spin::Lazy<Arc<Inode>> = spin::Lazy::new(|| Inode::dir());

/// /proc/self/maps（M13-01）：当前进程地址空间映射。
pub fn maps_inode() -> Arc<Inode> {
    let ino = Inode::file();
    *ino.data.lock() = crate::vmm::maps_content().into_bytes();
    ino
}

/// 挂载 overlay：lower = source 目录（只读），upper = 新建可写层。
pub fn mount_overlay(source: &str, target: &str) -> Result<(), i64> {
    if target == "/" || target.is_empty() {
        return Err(-16i64);
    }
    let tgt = resolve(target, false).map_err(|_| -2i64)?;
    if !tgt.is_dir() {
        return Err(-20i64);
    }
    if mount_entry(target).is_some() {
        return Err(-16i64);
    }
    let lower = resolve(source, false).map_err(|_| -2i64)?;
    if !lower.is_dir() {
        return Err(-20i64);
    }
    let upper = Inode::dir();
    *ACTIVE_OVERLAY.lock() = Some((String::from(target), upper.clone(), lower.clone()));
    MOUNTS.lock().push(Mount {
        path: String::from(target),
        kind: MountKind::Overlay { lower, upper },
    });
    Ok(())
}

/// 取路径 inode 元数据：(inode号, mode, nlink, size)。
pub fn stat_path(path: &str) -> Result<(u64, u32, u64, u64), i64> {
    let ino = resolve(path, false).map_err(|_| -2i64)?; // ENOENT
    Ok((
        Arc::as_ptr(&ino) as usize as u64,
        ino.mode,
        ino.nlink as u64,
        ino.size() as u64,
    ))
}

/// 路径解析：从根逐组件查找（仅绝对路径；"." 跳过，".." 简化不支持）。
/// 先查 dcache（FNV-1a 哈希，M4-切片3），miss 再扫父目录子项并回填。
/// 每深入一层检查挂载点（M4-切片4），命中则切换到挂载根子树。
/// `create_last`：末组件不存在时创建文件（仅当父目录存在）。
fn resolve(path: &str, create_last: bool) -> Result<Arc<Inode>, ()> {
    if !path.starts_with('/') {
        return Err(());
    }
    let mut cur = ROOT.clone();
    // overlay 遍历状态：(upper 当前目录(可 None), lower 当前目录(可 None))
    let mut ov: Option<(Option<Arc<Inode>>, Option<Arc<Inode>>)> = None;
    // proc 挂载内标记（M9-切片1：health/cpuinfo 特殊文件；M13-01：self/maps）
    let mut proc_active = false;
    let mut proc_self = false;
    let mut acc = String::from("/"); // 累积路径（挂载点匹配）
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    for (i, comp) in comps.iter().enumerate() {
        let last = i == comps.len() - 1;
        // 累积路径（带前导 "/"：i=0 时 acc 已是 "/"）
        if acc == "/" {
            acc.push_str(comp);
        } else {
            acc.push('/');
            acc.push_str(comp);
        }
        // 挂载点：命中则切换到挂载根，后续组件在挂载子树内解析
        if let Some(m) = mount_entry(&acc) {
            match m.kind {
                MountKind::Tmpfs(root) => {
                    cur = root;
                    ov = None;
                }
                MountKind::Overlay { lower, upper } => {
                    cur = upper.clone();
                    ov = Some((Some(upper), Some(lower)));
                }
                MountKind::Proc(root) => {
                    cur = root;
                    ov = None;
                    proc_active = true;
                }
            }
            continue;
        }
        // M9-切片1：/proc 特殊文件（health/cpuinfo）
        if proc_active {
            if let Some(ino) = proc_special_file(comp) {
                cur = ino;
                proc_active = false; // 文件不再有子组件
                continue;
            }
        }
        // M13-01：/proc/self → 当前任务视图目录
        if proc_active && *comp == "self" {
            cur = SELF_DIR.clone();
            proc_active = false;
            proc_self = true;
            continue;
        }
        // M13-01：/proc/self/maps → 当前进程地址空间映射
        if proc_self && *comp == "maps" {
            cur = maps_inode();
            proc_self = false;
            continue;
        }
        // overlay 内：upper 优先，其次 lower；.wh.* 白标记视为已删除（M7-切片2）
        if let Some((up, low)) = ov.take() {
            let marker = alloc::format!(".wh.{}", comp);
            if up.as_ref().map_or(false, |u| u.lookup(&marker).is_some()) {
                if last && create_last {
                    // 重建：移除 whiteout，在 upper 新建文件
                    let u = up.as_ref().unwrap();
                    u.children.lock().retain(|d| d.name != marker);
                    let ino = Inode::file();
                    u.insert_child(comp, ino.clone());
                    crate::dcache::dcache_insert(Arc::as_ptr(&u) as usize, comp, ino.clone());
                    cur = ino.clone();
                    ov = Some((Some(ino), low));
                } else {
                    return Err(());
                }
                continue;
            }
            let up_ino = up.as_ref().and_then(|u| u.lookup(comp));
            let low_ino = low.as_ref().and_then(|l| l.lookup(comp));
            if let Some(ino) = up_ino {
                cur = ino.clone();
                ov = Some((Some(ino), low.and_then(|l| l.lookup(comp))));
                continue;
            }
            if let Some(ino) = low_ino {
                cur = ino.clone();
                ov = Some((None, Some(ino)));
                continue;
            }
            if last && create_last {
                // 父目录在 upper 中直接建；仅存于 lower 时按需补建 upper 目录链
                let u = match up {
                    Some(u) => u,
                    None => {
                        let (pparent, _) = split_last(&acc);
                        match overlay_upper_dir(&pparent) {
                            Some(u) => u,
                            None => return Err(()),
                        }
                    }
                };
                let ino = Inode::file();
                u.insert_child(comp, ino.clone());
                crate::dcache::dcache_insert(Arc::as_ptr(&u) as usize, comp, ino.clone());
                cur = ino.clone();
                ov = Some((Some(ino), low));
            } else {
                return Err(());
            }
            continue;
        }
        let parent_ptr = Arc::as_ptr(&cur) as usize;
        // dcache 快查
        if let Some(ino) = crate::dcache::dcache_lookup(parent_ptr, comp) {
            cur = ino;
            continue;
        }
        match cur.lookup(comp) {
            Some(ino) => {
                crate::dcache::dcache_insert(parent_ptr, comp, ino.clone());
                cur = ino;
            }
            None => {
                if last && create_last {
                    let ino = Inode::file();
                    cur.insert_child(comp, ino.clone());
                    crate::dcache::dcache_insert(parent_ptr, comp, ino.clone());
                    cur = ino;
                } else {
                    return Err(());
                }
            }
        }
    }
    Ok(cur)
}

/// 拆分路径为 (父目录路径, 末组件名)。如 "/a/b" → ("/a", "b")，"/f" → ("/", "f")。
fn split_last(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return (String::from("/"), String::new());
    }
    match trimmed.rfind('/') {
        Some(0) => (String::from("/"), String::from(&trimmed[1..])),
        Some(idx) => (String::from(&trimmed[..idx]), String::from(&trimmed[idx + 1..])),
        None => (String::from("/"), String::from(trimmed)),
    }
}

/// 创建目录（父目录必须存在且为空路径不合法）。
/// M7-切片2：overlay 内目录创建进 upper（父仅在下层时按需补建 upper 链）。
pub fn create_dir(path: &str) -> Result<(), i64> {
    let (parent, leaf) = split_last(path);
    if leaf.is_empty() {
        return Err(-2i64); // ENOENT
    }
    if let Some((up_p, _low_p)) = overlay_parents(path) {
        let up = match up_p {
            Some(u) => u,
            None => overlay_upper_dir(&parent).ok_or(-2i64)?,
        };
        if up.lookup(&leaf).is_some() {
            return Err(-17i64); // EEXIST
        }
        let ino = Inode::dir();
        up.insert_child(&leaf, ino.clone());
        crate::dcache::dcache_insert(Arc::as_ptr(&up) as usize, &leaf, ino);
        return Ok(());
    }
    let p = resolve(&parent, false).map_err(|_| -2i64)?;
    if !p.is_dir() {
        return Err(-20i64); // ENOTDIR
    }
    if p.lookup(&leaf).is_some() {
        return Err(-17i64); // EEXIST
    }
    let ino = Inode::dir();
    p.insert_child(&leaf, ino.clone());
    crate::dcache::dcache_insert(Arc::as_ptr(&p) as usize, &leaf, ino);
    Ok(())
}

/// 删除路径：is_dir=true 走 rmdir（要求空目录），否则 unlink 文件。
/// M7-切片2：overlay 中删除 lower 文件 → 在 upper 打 whiteout 标记，lower 保持只读。
pub fn remove(path: &str, is_dir: bool) -> Result<(), i64> {
    let (parent, leaf) = split_last(path);
    if leaf.is_empty() {
        return Err(-2i64); // ENOENT
    }
    let p = resolve(&parent, false).map_err(|_| -2i64)?;
    if !p.is_dir() {
        return Err(-20i64); // ENOTDIR
    }
    // overlay：目标在下层 → 打 whiteout 标记（upper 有拷贝则一并移除）
    if let Some((up_p, low_p)) = overlay_parents(path) {
        let in_lower = low_p.as_ref().map_or(false, |l| l.lookup(&leaf).is_some());
        if in_lower {
            let up = match up_p {
                Some(u) => u,
                None => overlay_upper_dir(&parent).ok_or(-2i64)?,
            };
            let ino = low_p.as_ref().unwrap().lookup(&leaf).unwrap();
            if is_dir {
                if !ino.is_dir() {
                    return Err(-20i64); // ENOTDIR
                }
                if !ino.children.lock().is_empty() {
                    return Err(-39i64); // ENOTEMPTY
                }
            } else if ino.is_dir() {
                return Err(-21i64); // EISDIR
            }
            let mut ukids = up.children.lock();
            ukids.retain(|d| d.name != leaf); // 移除 upper 拷贝（如有）
            let marker = alloc::format!(".wh.{}", leaf);
            if !ukids.iter().any(|d| d.name == marker) {
                ukids.push(Dentry { name: marker, inode: Inode::file() });
            }
            drop(ukids);
            crate::dcache::dcache_remove(Arc::as_ptr(&p) as usize, &leaf);
            return Ok(());
        }
    }
    // 常规删除（upper 层独占文件、非 overlay 路径）
    let mut kids = p.children.lock();
    let idx = kids.iter().position(|d| d.name == leaf).ok_or(-2i64)?; // ENOENT
    let ino = kids[idx].inode.clone();
    if is_dir {
        if !ino.is_dir() {
            return Err(-20i64); // ENOTDIR
        }
        if !ino.children.lock().is_empty() {
            return Err(-39i64); // ENOTEMPTY
        }
    } else if ino.is_dir() {
        return Err(-21i64); // EISDIR
    }
    kids.remove(idx);
    drop(kids);
    // 同步失效 dcache 缓存项
    crate::dcache::dcache_remove(Arc::as_ptr(&p) as usize, &leaf);
    Ok(())
}

/// 枚举目录 inode（跳过前 skip 项），按 Linux dirent64 格式写入 buf。
/// 返回 (填充字节数, 写入项数)。
/// 布局：u64 d_ino | u64 d_off | u16 d_reclen | u8 d_type | char d_name[]（NUL 结尾）。
/// M7-切片2：overlay 目录走合并视图（隐藏 .wh.*）。
pub fn read_dir(ino: &Inode, skip: usize, buf: &mut [u8]) -> Result<(usize, usize), i64> {
    if !ino.is_dir() {
        return Err(-20i64); // ENOTDIR
    }
    let kids = visible_children(ino);
    let mut off = 0usize;
    let mut items = 0usize;
    for d in kids.iter().skip(skip) {
        let name_len = d.name.len() + 1; // + NUL
        let reclen = 19 + name_len;
        if off + reclen > buf.len() {
            break;
        }
        let rec = &mut buf[off..off + reclen];
        // d_ino：inode 指针低 48 位充当 inode 号（M3 简化）
        let ino_num = Arc::as_ptr(&d.inode) as usize as u64;
        rec[0..8].copy_from_slice(&ino_num.to_le_bytes());
        // d_off：下一项偏移（含本项）
        let next_off = (off + reclen) as u64;
        rec[8..16].copy_from_slice(&next_off.to_le_bytes());
        // d_reclen
        rec[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        // d_type：DT_DIR=4, DT_REG=8
        rec[18] = if d.inode.is_dir() { 4 } else { 8 };
        // d_name
        rec[19..19 + d.name.len()].copy_from_slice(d.name.as_bytes());
        off += reclen;
        items += 1;
    }
    Ok((off, items))
}

/// 打开路径。"/dev/uart" 走设备；目录返回 Dir；文件返回 Reg（O_CREAT 创建）。
/// 成功返回 File，失败返回负 errno。
/// 剥离 overlay 挂载路径前缀，返回相对组件。
fn overlay_rel<'a>(path: &'a str, mnt: &str) -> Vec<&'a str> {
    let rel = if let Some(rest) = path.strip_prefix(mnt) {
        rest
    } else {
        path
    };
    rel.split('/').filter(|c| !c.is_empty() && *c != ".").collect()
}

/// 判断路径是否解析到当前 overlay 的 lower 层文件（指针匹配）。
fn is_lower_file(path: &str, ino: &Arc<Inode>) -> bool {
    let ov = ACTIVE_OVERLAY.lock();
    let (mnt, _, lower) = match ov.as_ref() {
        Some(x) => x,
        None => return false,
    };
    // 沿相对组件在 lower 中定位，比较指针
    let comps = overlay_rel(path, mnt);
    let mut cur = lower.clone();
    for comp in &comps {
        match cur.lookup(comp) {
            Some(c) => cur = c,
            None => return false,
        }
    }
    Arc::ptr_eq(&cur, ino)
}

/// copy-up：把 overlay lower 中路径对应的文件拷贝到 upper（写时复制）。
pub fn copy_up(path: &str) -> Option<Arc<Inode>> {
    let ov = ACTIVE_OVERLAY.lock();
    let (mnt, upper, lower) = ov.as_ref()?;
    let comps = overlay_rel(path, mnt);
    if comps.is_empty() {
        return None;
    }
    // lower 定位源文件
    let mut lcur = lower.clone();
    for comp in &comps {
        lcur = lcur.lookup(comp)?;
    }
    if !lcur.is_file() {
        return None;
    }
    // upper 重建相对目录路径（缺失则创建），末组件建文件并拷贝数据
    let mut ucur = upper.clone();
    for comp in &comps[..comps.len() - 1] {
        match ucur.lookup(comp) {
            Some(c) if c.is_dir() => ucur = c,
            _ => {
                let d = Inode::dir();
                ucur.insert_child(comp, d.clone());
                ucur = d;
            }
        }
    }
    let leaf = comps[comps.len() - 1];
    if let Some(existing) = ucur.lookup(leaf) {
        return Some(existing); // 已在 upper（无需重复拷贝）
    }
    let f = Inode::file();
    *f.data.lock() = lcur.data.lock().clone();
    ucur.insert_child(leaf, f.clone());
    crate::dcache::dcache_insert(Arc::as_ptr(&ucur) as usize, leaf, f.clone());
    Some(f)
}

// ---- M7-切片2：whiteout + 合并视图 ----

/// 在 overlay 树中按指针定位 inode，返回相对根路径（DFS；demo 规模足够）。
fn find_in_tree(root: &Arc<Inode>, target: usize) -> Option<Vec<String>> {
    fn go(dir: &Arc<Inode>, target: usize, acc: &mut Vec<String>) -> bool {
        if Arc::as_ptr(dir) as usize == target {
            return true;
        }
        for d in dir.children.lock().iter() {
            if !d.inode.is_dir() {
                continue;
            }
            acc.push(d.name.clone());
            if go(&d.inode, target, acc) {
                return true;
            }
            acc.pop();
        }
        false
    }
    let mut acc = Vec::new();
    if go(root, target, &mut acc) {
        Some(acc)
    } else {
        None
    }
}

/// 沿相对路径导航。
fn walk_path(root: &Arc<Inode>, path: &[String]) -> Option<Arc<Inode>> {
    let mut cur = root.clone();
    for c in path {
        cur = cur.lookup(c)?;
    }
    Some(cur)
}

/// 返回 overlay 中 path 的父目录在 upper/lower 的 inode（仅限 overlay 内路径）。
/// 返回 (upper 父(可无), lower 父(可无))。
fn overlay_parents(path: &str) -> Option<(Option<Arc<Inode>>, Option<Arc<Inode>>)> {
    let ov = ACTIVE_OVERLAY.lock();
    let (mnt, upper, lower) = ov.as_ref()?;
    let (parent, _leaf) = split_last(path);
    let rel = match parent.strip_prefix(mnt) {
        Some("") => "",
        Some(rest) if rest.starts_with('/') => rest,
        _ => return None,
    };
    let comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    let mut up = Some(upper.clone());
    let mut low = Some(lower.clone());
    for comp in &comps {
        up = up.and_then(|u| u.lookup(comp).filter(|i| i.is_dir()));
        low = low.and_then(|l| l.lookup(comp).filter(|i| i.is_dir()));
    }
    Some((up, low))
}

/// 定位 parent 在 upper 中的对应目录；缺失时按需补建目录链（overlay 写时建 upper）。
fn overlay_upper_dir(parent: &str) -> Option<Arc<Inode>> {
    let ov = ACTIVE_OVERLAY.lock();
    let (mnt, upper, _lower) = ov.as_ref()?;
    let rel = match parent.strip_prefix(mnt) {
        Some("") => "",
        Some(rest) if rest.starts_with('/') => rest,
        _ => return None,
    };
    let comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    let mut ucur = upper.clone();
    for comp in &comps {
        match ucur.lookup(comp) {
            Some(c) if c.is_dir() => ucur = c,
            _ => {
                let d = Inode::dir();
                ucur.insert_child(comp, d.clone());
                ucur = d;
            }
        }
    }
    Some(ucur)
}

/// 合并视图目录子项：lower 起底 + upper 覆盖，隐藏 .wh.* 标记（M7-切片2）。
fn overlay_merged_children(ino: &Inode) -> Option<Vec<Dentry>> {
    let ov = ACTIVE_OVERLAY.lock();
    let (_, upper, lower) = ov.as_ref()?;
    let target = ino as *const Inode as usize;
    let (up_dir, low_dir) = if let Some(path) = find_in_tree(upper, target) {
        (walk_path(upper, &path), walk_path(lower, &path))
    } else if let Some(path) = find_in_tree(lower, target) {
        (None, walk_path(lower, &path))
    } else {
        return None;
    };
    let mut out: Vec<Dentry> = Vec::new();
    if let Some(ld) = low_dir {
        for d in ld.children.lock().iter() {
            if d.name.starts_with(".wh.") {
                continue;
            }
            // 该 lower 项在 upper 有 whiteout → 已删除，隐藏
            let whited = up_dir.as_ref().map_or(false, |u| {
                u.lookup(&alloc::format!(".wh.{}", d.name)).is_some()
            });
            if whited {
                continue;
            }
            out.push(Dentry { name: d.name.clone(), inode: d.inode.clone() });
        }
    }
    if let Some(ud) = up_dir {
        for d in ud.children.lock().iter() {
            if d.name.starts_with(".wh.") {
                continue;
            }
            match out.iter_mut().find(|x| x.name == d.name) {
                Some(x) => *x = Dentry { name: d.name.clone(), inode: d.inode.clone() },
                None => out.push(Dentry { name: d.name.clone(), inode: d.inode.clone() }),
            }
        }
    }
    Some(out)
}

/// 目录可见子项：proc 按 pid ns 动态合成；overlay 走合并视图；否则直接子项。
fn visible_children(ino: &Inode) -> Vec<Dentry> {
    // M8-切片4：/proc = 当前 pid ns 的任务视图（动态合成）
    if ino as *const Inode as usize == Arc::as_ptr(&PROC_ROOT) as usize {
        let ns = crate::task::current_pid_ns();
        let mut kids: Vec<Dentry> = crate::task::tasks_in_ns(ns)
            .iter()
            .map(|p| Dentry {
                name: alloc::format!("{}", p),
                inode: Inode::dir(), // /proc/<pid> 均为目录
            })
            .collect();
        // M13-01：/proc/self（当前任务视图）
        kids.push(Dentry { name: String::from("self"), inode: SELF_DIR.clone() });
        // M9-切片1：/proc/health + /proc/cpuinfo（只读文件）
        kids.push(Dentry { name: String::from("health"), inode: Inode::file() });
        kids.push(Dentry { name: String::from("cpuinfo"), inode: Inode::file() });
        return kids;
    }
    // M13-01：/proc/self 下仅 maps
    if ino as *const Inode as usize == Arc::as_ptr(&SELF_DIR) as usize {
        return alloc::vec![Dentry { name: String::from("maps"), inode: Inode::file() }];
    }
    if let Some(merged) = overlay_merged_children(ino) {
        return merged;
    }
    ino.children
        .lock()
        .iter()
        .map(|d| Dentry { name: d.name.clone(), inode: d.inode.clone() })
        .collect()
}

pub fn open_path(name: &str, flags: u64) -> Result<Arc<File>, i64> {
    if name == "/dev/uart" {
        return Ok(Arc::new(File::Uart));
    }
    let create = flags & O_CREAT != 0;
    let mut inode = resolve(name, create).map_err(|_| -2i64)?; // ENOENT
    // M7-切片1：overlay copy-up——写打开时若文件在下层，先拷到上层再写
    if inode.is_file() && (flags & (O_WRONLY | O_RDWR | O_TRUNC) != 0 || create) {
        if is_lower_file(name, &inode) {
            if let Some(up) = copy_up(name) {
                inode = up;
            }
        }
    }
    if inode.is_dir() {
        return Ok(Arc::new(File::Dir {
            inode,
            pos: Mutex::new(0),
        }));
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
