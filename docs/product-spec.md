# Find Profile 产品规格书

## 1. 产品定位

**产品名称**: Find Profile
**产品类型**: macOS 桌面文件检索工具

**一句话描述**: macOS 上的快速文件搜索工具，类 Windows Everything，支持索引挂载在 `/Volumes` 下的 NAS 存储。

**核心价值**:
- 快速定位本地和 NAS 存储中的文件
- 离线索引，无需实时扫描文件系统
- 支持 NAS 断开重连后保留历史索引

---

## 2. 核心用户场景

### 场景一：本地文件快速查找
用户在本地硬盘上有大量文件，需要快速找到某个文件或按名称筛选文件列表。传统 Finder 搜索速度慢，Find Profile 通过本地 SQLite FTS5 索引实现毫秒级响应。

### 场景二：NAS 文件检索
用户的工作文件存储在 NAS 上（挂载在 `/Volumes/` 路径），需要查找特定文件。NAS 文件无法使用 Spotlight 索引，Find Profile 支持扫描和索引 NAS 目录。

### 场景三：断网后的 NAS 索引访问
用户 NAS 临时断开，Find Profile 保留最后一次扫描的索引数据，用户仍可查看历史索引结果，不会因为 NAS 断开而丢失索引。

---

## 3. MVP 功能范围

| # | 功能 | 描述 |
|---|------|------|
| 1 | 添加本地目录 | 用户选择本地目录作为索引根目录 |
| 2 | 添加 NAS 目录 | 用户选择 `/Volumes` 下的 NAS 挂载点作为索引根目录 |
| 3 | 递归扫描目录 | 应用递归扫描目录树，遍历所有文件和子目录 |
| 4 | 文件元数据存储 | 保存：文件名、路径、扩展名、大小、修改时间、是否文件夹、来源类型 |
| 5 | SQLite FTS5 索引 | 使用全文搜索引擎建立文件名和路径索引 |
| 6 | 关键词搜索 | 搜索框输入关键词，快速返回匹配文件列表 |
| 7 | 结果表格展示 | 用表格列表展示搜索结果 |
| 8 | 打开文件 | 双击或快捷键打开选中文件 |
| 9 | 在 Finder 中显示 | 右键菜单或快捷键在 Finder 中定位文件 |
| 10 | 手动重新扫描 | 用户主动触发目录重新扫描 |
| 11 | 本地目录增量更新 | 本地目录使用文件系统事件（notify）实时增量更新索引 |
| 12 | NAS 定时扫描 | NAS 目录支持手动扫描和定时扫描，不依赖实时文件系统事件 |

---

## 4. 不做事项

1. **文件内容索引**: MVP 仅索引文件元数据，不解析文件内容
2. **Windows/Linux 支持**: MVP 仅支持 macOS
3. **实时文件系统事件 (NAS)**: NAS 目录不实现实时事件监听，仅支持手动/定时扫描
4. **文件预览**: 不实现文件预览功能
5. **文件操作**: 不实现文件复制、移动、删除等操作
6. **搜索结果排序策略**: MVP 仅返回匹配结果，不实现复杂排序
7. **多语言界面**: MVP 仅支持英文界面
8. **云端同步**: 不实现索引数据云端同步

---

## 5. 技术架构

### 技术栈
- **框架**: Tauri 2.x
- **后端**: Rust
- **前端**: React + TypeScript
- **数据库**: SQLite + FTS5
- **本地文件系统监控**: notify (Rust crate)
- **构建工具**: Vite

### 整体架构
```
┌─────────────────────────────────────────────────────────┐
│                    macOS Application                     │
├─────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────┐    │
│  │                  UI Layer (React)                │    │
│  │   - SearchBox  - ResultsTable  - Sidebar         │    │
│  └─────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────┤
│              Tauri Commands (IPC Bridge)                │
│   add_index_root / search_files / open_file / etc.      │
├─────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────┐    │
│  │               Backend Layer (Rust)               │    │
│  │   - Scanner    - Indexer    - FileWatcher        │    │
│  │   - Database   - Commands   - Models             │    │
│  └─────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────┐    │
│  │            Data Layer (SQLite + FTS5)           │    │
│  │   - roots  - files  - files_fts  - scan_jobs    │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

---

## 6. 数据库设计

### 表结构

#### roots
| 字段 | 类型 | 描述 |
|------|------|------|
| id | INTEGER PRIMARY KEY | 唯一标识 |
| path | TEXT NOT NULL UNIQUE | 目录路径 |
| root_type | TEXT NOT NULL | `local` 或 `nas` |
| status | TEXT NOT NULL | `available` / `disconnected` / `scanning` / `error` |
| created_at | INTEGER NOT NULL | 创建时间戳 |
| updated_at | INTEGER NOT NULL | 更新时间戳 |

#### files
| 字段 | 类型 | 描述 |
|------|------|------|
| id | INTEGER PRIMARY KEY | 唯一标识 |
| root_id | INTEGER NOT NULL | 关联 roots.id |
| name | TEXT NOT NULL | 文件名 |
| path | TEXT NOT NULL | 完整路径 |
| extension | TEXT | 文件扩展名 |
| size | INTEGER NOT NULL | 文件大小 (bytes) |
| is_dir | INTEGER NOT NULL | 是否文件夹 (0/1) |
| modified_at | INTEGER NOT NULL | 修改时间戳 |
| indexed_at | INTEGER NOT NULL | 索引时间戳 |

#### files_fts (FTS5 虚拟表)
- 全文搜索索引表，关联 files 表
- 索引字段: name, path

#### scan_jobs
| 字段 | 类型 | 描述 |
|------|------|------|
| id | INTEGER PRIMARY KEY | 唯一标识 |
| root_id | INTEGER NOT NULL | 关联 roots.id |
| status | TEXT NOT NULL | `pending` / `running` / `completed` / `failed` |
| started_at | INTEGER | 开始时间戳 |
| completed_at | INTEGER | 完成时间戳 |
| error_message | TEXT | 错误信息 |

#### settings
| 字段 | 类型 | 描述 |
|------|------|------|
| key | TEXT PRIMARY KEY | 配置键 |
| value | TEXT | 配置值 |

---

## 7. 模块拆分

### Rust 后端模块

| 模块 | 职责 |
|------|------|
| `db` | SQLite 连接管理、数据库初始化、迁移 |
| `models` | 数据结构定义 (Root, File, ScanJob, Settings) |
| `commands` | Tauri 命令处理 (add_index_root, search_files 等) |
| `scanner` | 文件系统扫描逻辑 |
| `indexer` | 文件索引写入 SQLite |
| `watcher` | 本地目录文件系统事件监控 (notify) |
| `scheduler` | NAS 定时扫描调度 |

### 前端模块

| 模块 | 职责 |
|------|------|
| `components/SearchBox` | 搜索输入框 |
| `components/ResultsTable` | 搜索结果表格 |
| `components/Sidebar` | 索引目录侧边栏 |
| `hooks/useSearch` | 搜索逻辑 Hook |
| `hooks/useRoots` | 索引目录管理 Hook |
| `api` | Tauri invoke 封装 |

---

## 8. 风险点

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| NAS 断开时扫描失败 | 扫描任务中断，索引数据不完整 | 扫描前检查 NAS 可用性，断开后保留旧数据 |
| 大目录扫描耗时过长 | 用户体验差，阻塞 UI | 后台线程扫描，进度实时同步到 UI |
| SQLite FTS5 索引重建慢 | 文件变化后索引更新不及时 | 增量更新，避免全量重建 |
| 文件系统事件丢失 | 本地目录增量更新不完整 | 定期全量校验，发现不一致时重新扫描 |
| 路径含特殊字符 | 搜索结果路径显示异常 | 统一使用 UTF-8 编码存储和显示 |
| NAS 挂载点命名变化 | 路径变化导致索引失效 | 记录 NAS 唯一标识，检测到变化时提示用户 |

---

## 9. 里程碑计划

### M1: 基础框架搭建
- [ ] Tauri 项目初始化
- [ ] SQLite 数据库初始化
- [ ] 基础 Tauri 命令骨架

### M2: 目录扫描功能
- [ ] 添加索引根目录 (add_index_root)
- [ ] 递归扫描目录
- [ ] 文件元数据入库

### M3: FTS5 索引和搜索
- [ ] 建立 files_fts 虚拟表
- [ ] 实现 search_files 命令
- [ ] 前端搜索界面

### M4: 结果展示和操作
- [ ] 表格展示搜索结果
- [ ] 实现 open_file 命令
- [ ] 实现 reveal_in_finder 命令

### M5: 本地目录文件监控
- [ ] 集成 notify 库
- [ ] 实现本地目录增量更新
- [ ] 文件变更事件处理

### M6: NAS 目录支持
- [ ] NAS 类型根目录识别 (path 以 /Volumes/ 开头)
- [ ] NAS 手动扫描
- [ ] NAS 定时扫描

### M7: 错误处理和稳定性
- [ ] NAS 断开处理
- [ ] 扫描错误恢复
- [ ] 用户友好错误提示

### M8: MVP 发布
- [ ] macOS 打包
- [ ] 功能验证
- [ ] 文档完善