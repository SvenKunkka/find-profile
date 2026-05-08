# Find Profile MVP 功能范围文档

## 1. MVP 概述

**MVP 目标**: 实现 macOS 文件快速检索工具的核心功能，支持本地目录和 NAS 挂载目录的索引与搜索。

**MVP 完成标准**:
- 用户可以添加本地目录和 NAS 目录作为索引根
- 目录扫描后，文件名和路径已建立 FTS5 索引
- 用户输入关键词可以在 1 秒内返回搜索结果
- 支持打开文件和 Finder 中显示
- 本地目录支持增量更新，NAS 目录支持手动/定时扫描

---

## 2. 功能范围

### 2.1 索引根目录管理

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 添加本地目录 | 用户选择本地目录，类型标记为 `local` | P0 |
| 添加 NAS 目录 | 用户选择 `/Volumes` 下的挂载点，类型标记为 `nas` | P0 |
| 列出所有根目录 | 侧边栏展示已添加的索引目录列表 | P0 |
| 移除根目录 | 删除根目录及其所有索引数据 | P0 |
| 根目录状态显示 | 显示 `available` / `disconnected` / `scanning` / `error` | P1 |

**路径类型判断逻辑**:
```rust
fn determine_root_type(path: &str) -> RootType {
    if path.starts_with("/Volumes/") {
        RootType::Nas
    } else {
        RootType::Local
    }
}
```

### 2.2 目录扫描

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 递归扫描目录 | 遍历目录下所有文件和子目录 | P0 |
| 收集文件元数据 | 采集: name, path, extension, size, is_dir, modified_at | P0 |
| 扫描进度显示 | 显示当前扫描目录和文件数量 | P1 |
| 取消扫描 | 用户可取消正在进行的扫描 | P2 |
| 重新扫描 | 对已扫描目录重新获取最新文件列表 | P0 |

**扫描流程**:
```
用户点击 "扫描" / 定时任务触发
    │
    ▼
更新 root.status = "scanning"
    │
    ▼
创建 scan_job (status = "running")
    │
    ▼
Scanner 递归遍历目录
    │
    ▼
Indexer 批量写入数据库
    │
    ▼
更新 scan_job.status = "completed"
    │
    ▼
更新 root.status = "available"
```

### 2.3 文件索引

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 文件元数据存储 | name, path, extension, size, is_dir, modified_at, indexed_at, root_id | P0 |
| FTS5 全文索引 | 对 name 和 path 建立全文搜索索引 | P0 |
| 增量更新 (本地) | 文件监控事件触发，更新索引 | P0 |
| 定时扫描 (NAS) | 可配置时间间隔自动扫描 NAS 目录 | P1 |

**索引文件元数据字段**:

| 字段 | 类型 | 说明 |
|------|------|------|
| id | INTEGER PRIMARY KEY | 自增 ID |
| root_id | INTEGER | 关联 roots.id |
| name | TEXT NOT NULL | 文件名 |
| path | TEXT NOT NULL | 完整路径 |
| extension | TEXT | 文件扩展名 |
| size | INTEGER | 文件大小 (bytes) |
| is_dir | INTEGER | 0=文件, 1=文件夹 |
| modified_at | INTEGER | 修改时间戳 |
| indexed_at | INTEGER | 索引时间戳 |

### 2.4 文件搜索

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 关键词搜索 | 输入文件名关键词，返回匹配结果 | P0 |
| 前缀匹配 | 支持 `keyword*` 形式的模糊搜索 | P0 |
| 搜索结果排序 | 按相关度或修改时间排序 | P1 |
| 搜索结果分页 | 大结果集分页展示 | P2 |
| 清空搜索 | 清空搜索框和结果列表 | P0 |

**搜索 SQL 示例**:
```sql
SELECT f.* FROM files f
JOIN files_fts fts ON f.id = fts.rowid
WHERE files_fts MATCH 'keyword*'
ORDER BY f.modified_at DESC
LIMIT 100
```

### 2.5 结果展示

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 表格列表展示 | 列: 名称, 路径, 扩展名, 大小, 修改时间 | P0 |
| 路径列显示 | 显示文件完整路径 | P0 |
| 按列排序 | 点击表头排序 | P1 |
| 行数统计 | 显示搜索结果总数 | P0 |
| 空结果提示 | 无匹配结果时显示提示 | P0 |

### 2.6 文件操作

| 功能 | 描述 | 优先级 |
|------|------|--------|
| 打开文件 | 双击或回车打开选中文件 | P0 |
| 在 Finder 中显示 | 右键菜单 "Show in Finder" | P0 |
| 复制路径 | 复制文件完整路径到剪贴板 | P1 |

---

## 3. 用户界面

### 3.1 窗口布局

```
┌────────────────────────────────────────────────────────────┐
│  Find Profile                                    [─][□][×] │
├────────────────────────────────────────────────────────────┤
│  ┌────────────────────────────────────────────────────────┐ │
│  │  🔍  Search files...                                  │ │
│  └────────────────────────────────────────────────────────┘ │
├────────────────────────────────────────────────────────────┤
│  ┌──────────────┬───────────────────────────────────────┐ │
│  │ INDEX ROOTS  │  Results (142 items)                  │ │
│  │              │  ────────────────────────────────────│ │
│  │ + Add Local  │  Name          │ Size │ Modified     │ │
│  │ + Add NAS    │  ─────────────────────────────────────│ │
│  │              │  document.pdf   │ 2.3MB│ 2024-01-15   │ │
│  │ ● ~/Docs     │  notes.txt     │ 4KB  │ 2024-01-14   │ │
│  │ ● /Volumes/NAS│ image.png      │ 156KB│ 2024-01-10   │ │
│  │              │  ...                                 │ │
│  └──────────────┴───────────────────────────────────────┘ │
├────────────────────────────────────────────────────────────┤
│  Ready                                        v0.1.0       │
└────────────────────────────────────────────────────────────┘
```

### 3.2 组件清单

| 组件 | 描述 | 状态 |
|------|------|------|
| SearchBox | 搜索输入框，带搜索图标 | 待开发 |
| ResultsTable | 搜索结果表格，多列展示 | 待开发 |
| Sidebar | 索引目录侧边栏，管理根目录 | 待开发 |
| StatusBar | 底部状态栏，显示状态和版本 | 待开发 |
| AddRootDialog | 添加根目录对话框 | 待开发 |
| ContextMenu | 右键菜单 (Open, Show in Finder) | 待开发 |

---

## 4. 数据存储

### 4.1 SQLite 表

```sql
-- 索引根目录
CREATE TABLE roots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    root_type TEXT NOT NULL CHECK(root_type IN ('local', 'nas')),
    status TEXT NOT NULL DEFAULT 'available'
        CHECK(status IN ('available', 'disconnected', 'scanning', 'error')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 文件索引
CREATE TABLE files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    extension TEXT,
    size INTEGER NOT NULL DEFAULT 0,
    is_dir INTEGER NOT NULL DEFAULT 0,
    modified_at INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL
);

-- FTS5 全文索引
CREATE VIRTUAL TABLE files_fts USING fts5(
    name,
    path,
    content='files',
    content_rowid='id'
);

-- 扫描任务
CREATE TABLE scan_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'running', 'completed', 'failed')),
    started_at INTEGER,
    completed_at INTEGER,
    error_message TEXT
);

-- 应用设置
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT
);

-- 索引
CREATE INDEX idx_files_root_id ON files(root_id);
CREATE INDEX idx_files_name ON files(name);
CREATE INDEX idx_scan_jobs_root_id ON scan_jobs(root_id);
```

### 4.2 FTS5 触发器

```sql
-- 插入触发器
CREATE TRIGGER files_ai AFTER INSERT ON files BEGIN
    INSERT INTO files_fts(rowid, name, path) VALUES (new.id, new.name, new.path);
END;

-- 删除触发器
CREATE TRIGGER files_ad AFTER DELETE ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name, path) VALUES('delete', old.id, old.name, old.path);
END;

-- 更新触发器
CREATE TRIGGER files_au AFTER UPDATE ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name, path) VALUES('delete', old.id, old.name, old.path);
    INSERT INTO files_fts(rowid, name, path) VALUES (new.id, new.name, new.path);
END;
```

---

## 5. Tauri Commands

| 命令 | 参数 | 返回值 | 描述 |
|------|------|--------|------|
| `add_index_root` | `path: String` | `Root` | 添加索引根目录 |
| `list_index_roots` | - | `Vec<Root>` | 列出所有根目录 |
| `remove_index_root` | `id: i64` | `()` | 删除根目录 |
| `start_scan` | `root_id: i64` | `()` | 启动扫描 |
| `get_scan_status` | `root_id: i64` | `ScanJob` | 获取扫描状态 |
| `search_files` | `query: String, limit: Option<usize>` | `Vec<File>` | 搜索文件 |
| `open_file` | `path: String` | `()` | 打开文件 |
| `reveal_in_finder` | `path: String` | `()` | 在 Finder 中显示 |
| `rescan_root` | `root_id: i64` | `()` | 重新扫描 |

---

## 6. 非功能需求

### 6.1 性能

| 指标 | 目标 |
|------|------|
| 搜索响应时间 | < 1 秒 (1000 条结果内) |
| 10 万文件扫描时间 | < 5 分钟 |
| 应用启动时间 | < 3 秒 |
| 内存占用 | < 200 MB (idle) |

### 6.2 可靠性

| 场景 | 处理方式 |
|------|----------|
| NAS 断开 | 保留索引，标记为 disconnected |
| 扫描中断 | 保留已完成部分，标记 failed |
| 数据库损坏 | 提示用户重建索引 |

### 6.3 兼容性

| 项目 | 要求 |
|------|------|
| macOS 版本 | macOS 11.0+ |
| 芯片架构 | Intel 和 Apple Silicon |

---

## 7. 发布检查清单

- [ ] 所有 P0 功能测试通过
- [ ] 搜索响应时间 < 1 秒
- [ ] 本地目录增量更新正常工作
- [ ] NAS 目录手动扫描正常工作
- [ ] 打开文件和 Finder 显示功能正常
- [ ] macOS 打包成功
- [ ] 应用可正常启动和关闭
- [ ] 无崩溃和严重错误