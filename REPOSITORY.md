# 山水观心操作系统软件仓库规范（山水观心操作系统 Repository System, NRS）

> 本文档定义 山水观心操作系统官方软件仓库的**完整技术规范**：本地文件规范、后端服务器协议、CLI 操作流程。
> 设计哲学与三阶段演进见 [DESIGN.md §22](DESIGN.md)，长期定位见 [DESIGN_EXTENSION.md](DESIGN_EXTENSION.md)。
>
> 核心思路：**"极简、防呆、离线优先"** —— 复用 `sources.list + apt`（Debian）、`tap`（Homebrew）、
> `pacman.d`（Arch）等 30 年工程验证的成熟模式，但不照搬完整包管理器（apt 太重），
> 而是元数据索引（`index.json`）+ 签名验证的轻量方案，后端代码量 ≤500 行。

---

## 1. 设计原则

针对嵌入式"内存极小、网络可能不稳定"的特性：

| 原则 | 含义 |
|---|---|
| **极简** | 后端只需 2 个静态 HTTP 端点，无动态后台、无数据库，可纯静态托管 |
| **防呆** | `softwares.list` 不是"下载列表"而是"策略清单"；核心配置与用户配置分离 |
| **离线优先** | 元数据 `index.json` 本地缓存；`shanshui-guanxin search` 只扫缓存，不联网 |
| **签名验证** | 所有包 + 索引均带 Ed25519 签名，设备端公钥验签后才解压/合并 |
| **内置最佳实践** | 元数据带 `memory_required` / `config_template`，安装时主动提示并默认应用 |

---

## 2. 本地文件规范

拆分为两个文件，职责分明，避免用户误操作弄坏核心配置。

### 2.1 `/etc/shanshui-guanxin/repos.list`（系统级，只读，官方 OTA 更新）

定义官方仓库地址和 GPG 公钥指纹，随系统升级更新，用户一般不改。

```
# 格式：类型 名称  URL  组件  优先级
core official https://repo.shanshui-guanxin-os.com/ stable 100
```

### 2.2 `/etc/shanshui-guanxin/softwares.list`（用户级，可编辑）

用户可编辑的**策略清单**——锁定版本 / 添加第三方源 / 屏蔽软件，非"下载列表"。

```
# 格式：软件包名  版本约束  来源标记
# 强制从官方源安装特定版本
redis   =7.2.4   @official
# 从第三方社区源安装
mosquitto  latest  @community
# 屏蔽某个软件（前面 "-" 表示禁止安装，防 OTA 意外安装）
-python3
```

真正的软件元数据（版本号、下载 URL、SHA256、依赖、内存预算标签）**不写死在此文件**，
由 `shanshui-guanxin update` 从后端拉取到本地缓存。

---

## 3. 后端服务器协议

后端仅需 **2 个静态 HTTP 端点**，无数据库查询逻辑。

### 3.1 元数据索引 `index.json`

```json
{
  "version": 1,
  "timestamp": "2026-08-27T10:00:00Z",
  "signature": "base64_ed25519_signature...",
  "packages": {
    "redis": {
      "versions": [
        {
          "version": "7.2.4",
          "arch": "x86_64-musl",
          "size": 2450000,
          "sha256": "abc123...",
          "url": "https://repo.shanshui-guanxin-os.com/pool/redis-7.2.4-x86_64-musl.tar.gz",
          "memory_required": "64MB",
          "dependencies": ["musl>=1.2.0"],
          "config_template": "maxmemory 64mb"
        }
      ]
    }
  }
}
```

### 3.2 软件包归档 `pool/` 目录

存放预编译、完全静态链接的 musl 二进制 tarball（二进制 + 默认配置 + init 脚本模板）。
每个包附带 `.sig` 签名文件，设备端验证签名后才解压。

**为什么这样设计**：
- **纯静态**：后端可为 S3 / Nginx / GitHub Releases，零运维成本；
- **体积小**：`index.json` 压缩后仅几十 KB，适合不定期 OTA 拉取；
- **安全**：即使 `index.json` 被篡改，设备端公钥验签直接拒绝。

---

## 4. 用户操作流程（CLI 交互）

用户与仓库的交互通过 `shanshui-guanxin` 命令完成，彻底隐藏底层文件细节。

| 用户操作 | 命令行 | 背后发生 |
|---|---|---|
| 查看推荐软件 | `shanshui-guanxin search redis` | 扫描本地 `index.json` 缓存，列出匹配项及版本/内存要求 |
| 安装软件 | `shanshui-guanxin install redis` | ① 检查 `softwares.list` 锁定版本；② 下载 tarball 验证 SHA256+签名；③ 解压到 `/opt/shanshui-guanxin/packages/redis/`；④ 生成 OverlayFS 层，作为只读 lower 挂载到容器 |
| 手动编辑策略 | `shanshui-guanxin edit softwares` | 调用 `$EDITOR` 打开 `softwares.list`，保存后自动校验格式 |
| 更新软件包列表 | `shanshui-guanxin update` | 请求所有 `repos.list` 源，合并 `index.json` 到本地缓存，验证签名 |
| 添加第三方源 | `shanshui-guanxin repo add community https://repo.my-company.com/` | 自动写入 `softwares.list`，立即 `shanshui-guanxin update` 拉取元数据 |

---

## 5. 致命 Bug 的自动防护机制

直接解决 DESIGN §21 预判的用户痛点：

| 用户痛点 | 仓库自动防御机制 |
|---|---|
| 装错 glibc 版本致段错误 | 仓库二进制强制 `arch == x86_64-musl`；`shanshui-guanxin install` 下载前读 `index.json` 的 `arch` 字段，不匹配直接报错终止 |
| Redis 内存配置错误致 OOM | `config_template` 预置 `maxmemory 64mb`；安装时询问"是否应用官方推荐配置(64MB)？"默认选"是" |
| 安装包太大撑爆系统 | `index.json` 带 `memory_required` 标签；安装前 `shanshui-guanxin check` 评估剩余内存，不足则警告"需 64MB，当前仅剩 40MB，继续可能 OOM，是否继续？" |

---

## 6. 极简后端搭建（初期无需写后端代码）

用纯静态方案即可跑通：

1. **托管**：GitHub Releases 或 Cloudflare R2 存储桶；
2. **元数据生成**：本地 Python/Shell 脚本扫描 `pool/` 目录，自动生成 `index.json` 并签名；
3. **发布流程**：新增包 → 运行脚本 → 生成新 `index.json` → 上传存储桶覆盖。

`softwares.list` 指向的 URL 就是静态 `https://.../index.json`，后端代码量近乎为零，效果与完整仓库一致。

---

## 7. 与行业标准对齐

| 本方案 | 对齐的成熟模式 |
|---|---|
| `softwares.list` 用户可编辑 + 后端服务器 | Debian `sources.list + apt`、VSCode `extensions.json + marketplace`、Docker Hub + 镜像仓库 |
| `index.json` 元数据索引 | Homebrew `Formula`、Arch `pacman` 包数据库 |
| Ed25519 签名验证 | Debian `Release.gpg`、Arch `pacman-key` |

> 结论：`softwares.list + 后端服务器` 是经 30 年验证的成熟模式；补上"元数据索引 + 签名验证"
> 后，山水观心操作系统软件仓库从"一个文件列表"升级为"完整、安全、离线、可审计的应用交付平台"，
> 代码量（含后端）不超过 500 行——这是整个项目性价比最高的基础设施之一。
