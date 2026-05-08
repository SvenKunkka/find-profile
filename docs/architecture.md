# Find Profile 技术架构文档

## 1. 架构概述

Find Profile 是基于 Tauri 2.x 框架的 macOS 桌面应用，采用 Rust 后端 + React 前端的架构。前端负责用户交互和结果展示，后端负责文件系统扫描、索引管理和数据库操作。

```
┌─────────────────────────────────────────────────────────────┐
│                      macOS Desktop App                        │
│                        Find Profile                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   ┌─────────────────────────────────────────────────────┐   │
│   │                   React Frontend                      │   │
│   │  ┌─────────┐  ┌─────────────┐  ┌─────────────────┐   │   │
│   │  │SearchBox│  │ResultsTable │  │    Sidebar      │   │   │
│   │  └─────────┘  └─────────────┘  └─────────────────┘   │   │
│   └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                   Tauri IPC (invoke)                         │
│                            ▼                                 │
│   ┌─────────────────────────────────────────────────────┐   │
│   │                  Rust Backend                         │   │
│   │  ┌───────────┐  ┌───────────┐  ┌─────────────────┐   │   │
│   │  │ Commands  │  │  Scanner  │  │    Indexer      │   │   │
│   │  └───────────┘  └───────────┘  └─────────────────┘   │   │
│   │  ┌───────────┐  ┌───────────┐  ┌─────────────────┐   │   │
│   │  │  Watcher  │  │ Scheduler │  │    Database     │   │   │
│   │  └───────────┘  └───────────┘  └─────────────────┘   │   │
│   └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            ▼                                 │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              SQLite + FTS5 Database                  │   │
│   │      (roots, files, files_fts, scan_jobs)          │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. 技术选型

| 层级 | 技术 | 版本 | 用途 |
|------|------|------|------|
| 跨平台框架 | Tauri | 2.x | 桌面应用框架 |
| 后端语言 | Rust | stable | 后端逻辑 |
| 前端框架 | React | 18.x | UI 组件 |
| 前端语言 | TypeScript | 5.6.x | 类型安全 |
| 构建工具 | Vite | 6.x | 前端构建 |
| 数据库 | SQLite | - | 本地数据库 |
| 全文搜索 | FTS5 | - | 全文索引 |
| 文件监控 | notify | 6.x | 本地文件事件 |
| 时间处理 | chrono | 0.4 | 时间戳转换 |
| 异步运行时 | tokio | 1.x | 异步任务 |

---

## 3. 模块设计

### 3.1 Commands 模块 (`src-tauri/src/commands/`)

处理前端请求的 Tauri 命令。

| 命令 | 签名 | 描述 |
|------|------|------|
| `add_index_root` | `(path: String) -> Result<Root>` | 添加索引根目录 |
| `list_index_roots` | `() -> Vec<Root>` | 列出所有索引根目录 |
| `remove_index_root` | `(id: i64) -> Result<()>` | 移除索引根目录 |
| `start_scan` | `(root_id: i64) -> Result<()>` | 启动扫描任务 |
| `get_scan_status` | `(root_id: i64) -> Result<ScanJob>` | 获取扫描状态 |
| `search_files` | `(query: String) -> Vec<File>` | 搜索文件 |
| `open_file` | `(path: String) -> Result<()>` | 打开文件 |
| `reveal_in_finder` | `(path: String) -> Result<()>` | 在 Finder 中显示 |
| `rescan_root` | `(root_id: i64) -> Result<()>` | 重新扫描根目录 |

### 3.2 Scanner 模块 (`src-tauri/src/scanner/`)

文件系统的递归扫描逻辑。

```
Scanner
├── walk_dir(path)     # 递归遍历目录
├── collect_metadata() # 收集文件元数据
└── emit file events  # 发送文件事件给 Indexer
```

**关键类型**:
```rust
struct FileMetadata {
    name: String,
    path: String,
    extension: Option<String>,
    size: u64,
    is_dir: bool,
    modified_at: i64,
}
```

### 3.3 Indexer 模块 (`src-tauri/src/indexer/`)

将文件元数据写入 SQLite 数据库。

```
Indexer
├── insert_file()       # 插入单个文件记录
├── insert_batch()      # 批量插入文件记录
├── update_file()       # 更新文件记录
├── delete_by_root()    # 删除根目录下所有文件
└── rebuild_fts()       # 重建 FTS5 索引
```

### 3.4 Watcher 模块 (`src-tauri/src/watcher/`)

使用 `notify` 库监控本地目录的文件系统事件。

```
Watcher
├── watch_root()        # 开始监控目录
├── unwatch_root()      # 停止监控目录
└── handle events       # 处理 CREATE/MODIFY/DELETE 事件
```

**事件类型映射**:
| notify 事件 | Indexer 操作 |
|------------|--------------|
| Create | insert_file |
| Modify | update_file |
| Delete | delete_file |

### 3.5 Scheduler 模块 (`src-tauri/src/scheduler/`)

NAS 定时扫描调度器。

```
Scheduler
├── start_interval()    # 启动定时任务
├── stop_interval()      # 停止定时任务
└── trigger_scan()      # 触发扫描
```

### 3.6 Database 模块 (`src-tauri/src/db/`)

SQLite 数据库管理。

```
Database
├── init()              # 初始化数据库和表
├── get_connection()    # 获取数据库连接
└── run_migrations()   # 执行数据迁移
```

---

## 4. 数据流

### 4.1 添加索引根目录

```
用户点击 "添加目录"
    │
    ▼
前端调用 add_index_root
    │
    ▼
Commands: add_index_root(path)
    │
    ├──► 检查路径是否有效
    │
    ├──► 判断 root_type (local / nas)
    │
    ├──► 插入 roots 表
    │
    └──► 返回 Root 对象
```

### 4.2 文件扫描流程

```
用户触发扫描 / 定时任务触发
    │
    ▼
Scanner: walk_dir(path)
    │
    ├──► 读取目录条目
    │
    ├──► 对每个文件/目录:
    │       ├──► 收集 FileMetadata
    │       └──► 发送到 Indexer
    │
    ├──► 递归处理子目录
    │
    └──► 完成遍历
    │
    ▼
Indexer: insert_batch(files)
    │
    ├──► 开启事务
    │
    ├──► 清空该 root_id 旧数据 (可选: 增量更新)
    │
    ├──► 批量插入 files
    │
    ├──► 更新 files_fts 索引
    │
    └──► 提交事务
    │
    ▼
更新 scan_jobs 状态为 completed
```

### 4.3 搜索流程

```
用户输入关键词并回车
    │
    ▼
前端调用 search_files(query)
    │
    ▼
Commands: search_files(query)
    │
    ├──► 构建 FTS5 查询
    │
    │    SELECT f.* FROM files f
    │    JOIN files_fts fts ON f.id = fts.rowid
    │    WHERE files_fts MATCH 'keyword*'
    │
    ├──► 执行查询
    │
    └──► 返回 Vec<File>
    │
    ▼
前端更新 ResultsTable
```

### 4.4 本地目录文件监控流程

```
Watcher 启动，监控本地根目录
    │
    ▼
文件系统事件发生 (notify)
    │
    ├──► CREATE ──────► Indexer.insert_file()
    │
    ├──► MODIFY ──────► Indexer.update_file()
    │
    └──► DELETE ──────► Indexer.delete_file()
    │
    ▼
实时更新 SQLite 数据库
```

---

## 5. 目录结构

```
find-profile/
├── src/                          # 前端源码
│   ├── main.ts                   # 前端入口
│   ├── styles.css
│   └── assets/
│
├── src-tauri/                    # Rust 后端源码
│   ├── src/
│   │   ├── main.rs              # 程序入口
│   │   ├── lib.rs               # 库入口
│   │   ├── commands/            # Tauri 命令
│   │   │   ├── mod.rs
│   │   │   ├── roots.rs
│   │   │   ├── scan.rs
│   │   │   ├── search.rs
│   │   │   └── file_ops.rs
│   │   ├── db/                  # 数据库模块
│   │   │   ├── mod.rs
│   │   │   └── migrations.rs
│   │   ├── models/              # 数据模型
│   │   │   ├── mod.rs
│   │   │   ├── root.rs
│   │   │   ├── file.rs
│   │   │   └── scan_job.rs
│   │   ├── scanner/             # 文件扫描
│   │   │   └── mod.rs
│   │   ├── indexer/             # 索引写入
│   │   │   └── mod.rs
│   │   ├── watcher/             # 文件监控
│   │   │   └── mod.rs
│   │   └── scheduler/           # 定时调度
│   │       └── mod.rs
│   ├── Cargo.toml
│   ├── build.rs
│   └── tauri.conf.json
│
├── docs/                         # 文档
│   ├── product-spec.md
│   ├── architecture.md
│   └── mvp-scope.md
│
└── package.json
```

---

## 6. Tauri 配置

```json
// src-tauri/tauri.conf.json
{
  "productName": "find-profile",
  "version": "0.1.0",
  "identifier": "com.keychron.find-profile",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "withGlobalTauri": true,
    "windows": [
      {
        "title": "find-profile",
        "width": 800,
        "height": 600
      }
    ]
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/32x32.png", "icons/icon.icns"]
  }
}
```

---

## 7. 错误处理策略

| 场景 | 处理方式 |
|------|----------|
| NAS 路径不可访问 | 标记 root status 为 `disconnected`，保留旧数据 |
| 扫描过程中目录被删除 | 标记 scan_jobs status 为 `failed`，记录 error_message |
| 数据库写入失败 | 回滚事务，返回错误给前端 |
| 文件路径含特殊字符 | UTF-8 标准化处理 |
| 大量文件扫描内存占用 | 分批处理，限制单批次大小 |

---

## 8. 性能考量

| 优化点 | 方案 |
|--------|------|
| 批量插入 | 使用 `insert_batch` 减少数据库往返 |
| FTS5 查询 | 使用前缀匹配 `keyword*` |
| 大目录扫描 | 后台线程执行，避免阻塞 UI |
| 内存占用 | 流式处理，边扫描边写入 |
| 增量更新 | 文件监控事件驱动，避免全量重建 |