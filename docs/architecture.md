# Find Profile · 架构设计 (Architecture)

## 1. 总览

Find Profile 是一个 **Tauri 2** 桌面应用：

- **前端**：React + TypeScript（Vite 构建）
- **后端**：Rust（在 Tauri 进程内运行）
- **存储**：本地 SQLite（启用 FTS5 扩展）
- **OS 集成**：依赖 macOS 已经把 NAS 挂载在 `/Volumes/<volume-name>`，应用本身不实现 SMB/NFS 协议

应用是单进程双侧结构：
- 前端只做 UI 渲染和事件处理。
- 所有索引、扫描、查询、文件系统调用都在 Rust 后端完成。
- 前端通过 `@tauri-apps/api` 的 `invoke()` 调用后端命令。

## 2. 高层架构图

```
┌────────────────────────────────────────────────────────────┐
│                      Find Profile (Tauri)                  │
│                                                            │
│  ┌──────────────────────────┐    invoke()                  │
│  │  Frontend (WebView)      │ ─────────────┐               │
│  │  React + TypeScript      │              │               │
│  │  - Search box            │              ▼               │
│  │  - Roots panel           │   ┌────────────────────────┐ │
│  │  - Results list          │   │ Tauri Commands (Rust)  │ │
│  │  - Status bar            │   │  add_index_root         │ │
│  └──────────────────────────┘   │  list_index_roots       │ │
│                                 │  start_scan             │ │
│                                 │  get_scan_status        │ │
│                                 │  search_files           │ │
│                                 │  open_file              │ │
│                                 │  reveal_in_finder       │ │
│                                 │  remove_index_root      │ │
│                                 │  rescan_root            │ │
│                                 └──────────┬─────────────┘  │
│                                            │                │
│            ┌───────────────────────────────┼─────────────┐  │
│            ▼                               ▼             ▼  │
│  ┌──────────────────┐           ┌──────────────────┐  ┌────┐│
│  │ Indexer / Scanner│           │ Search Engine    │  │ FS ││
│  │ (walkdir, async) │           │ (SQLite + FTS5)  │  │ I/O││
│  └────────┬─────────┘           └────────┬─────────┘  └────┘│
│           │                              │                  │
│           ▼                              ▼                  │
│      ┌──────────────────────────────────────┐               │
│      │   SQLite database (find_profile.db)  │               │
│      │   tables: roots, files, files_fts,    │              │
│      │           scan_jobs, settings         │              │
│      └──────────────────────────────────────┘               │
└────────────────────────────────────────────────────────────┘
```

## 3. 进程与线程模型

- 单一 Tauri 应用进程。
- Rust 侧使用一个**异步运行时**（Tokio，由 Tauri 提供）。
- 扫描任务在后台 task 中执行，避免阻塞 invoke handler。
- SQLite 通过单一连接池 / 串行写访问；读取可并发。
- 前端通过 `invoke` 触发命令；扫描进度通过 Tauri Event（`emit`）回推到前端。

## 4. 模块划分（Rust）

建议在 `src-tauri/src/` 下逐步拆出以下模块（MVP 演进过程中按需创建，不一次到位）：

| 模块 | 职责 |
| --- | --- |
| `main.rs` | 二进制入口，调用 `lib::run()` |
| `lib.rs` | 注册 Tauri commands、初始化 DB、构建 Builder |
| `db.rs` | SQLite 连接、迁移、schema 初始化 |
| `model.rs` | `Root`, `FileRecord`, `ScanJob` 等数据结构 |
| `roots.rs` | 索引根管理：添加 / 列表 / 删除 / 路径 kind 检测 |
| `scanner.rs` | 递归扫描、提取元数据、写入 DB |
| `search.rs` | FTS5 查询、排序、分页 |
| `fs_ops.rs` | 打开文件、在 Finder 中显示 |
| `events.rs` | Tauri 事件定义（如 `scan-progress`, `scan-finished`） |

> MVP 早期完全可以全部塞在 `lib.rs` 里，按需拆分即可。

## 5. 模块划分（前端）

`src/` 下计划逐步建立（当前仍是 vanilla TS 模板）：

```
src/
├── main.tsx              # React 入口
├── App.tsx               # 顶层布局（标题 / 搜索框 / 根面板 / 结果 / 状态栏）
├── api/
│   └── tauri.ts          # 包装 invoke()，统一 try/catch 与类型
├── components/
│   ├── SearchBar.tsx
│   ├── RootsPanel.tsx
│   ├── ResultsList.tsx
│   └── StatusBar.tsx
├── hooks/
│   └── useScanProgress.ts
├── types.ts              # 与 Rust 共享的 DTO 类型
└── styles.css
```

## 6. 数据模型 (SQLite)

数据库文件：`<app-data>/find_profile.db`（macOS 下通常位于 `~/Library/Application Support/com.keychron.find-profile/`）。

### 6.1 表 `roots`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | INTEGER PK | 自增 |
| `path` | TEXT UNIQUE | 根的绝对路径（不含尾随 `/`） |
| `kind` | TEXT | `local` / `nas`（用户可覆盖自动检测） |
| `status` | TEXT | `available` / `disconnected` / `scanning` / `error` |
| `last_scan_at` | INTEGER | Unix 秒，最后一次成功扫描完成时间 |
| `last_error` | TEXT NULL | 最近一次错误描述（可空） |
| `created_at` | INTEGER | 添加时间 |

### 6.2 表 `files`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | INTEGER PK | 自增 |
| `root_id` | INTEGER FK | 关联 `roots.id` |
| `path` | TEXT | 完整绝对路径 |
| `parent_dir` | TEXT | 所在目录 |
| `name` | TEXT | 文件名（含扩展名） |
| `ext` | TEXT | 扩展名（小写，无点） |
| `size` | INTEGER | 字节数 |
| `mtime` | INTEGER | 修改时间（Unix 秒） |
| `is_dir` | INTEGER | 0/1 |
| `last_seen_scan` | INTEGER | 最近被哪次扫描看到（FK `scan_jobs.id`） |

> 索引：`(root_id, path)` 唯一；`name` 和 `parent_dir` 单列普通索引备用。

### 6.3 虚拟表 `files_fts`

```sql
CREATE VIRTUAL TABLE files_fts USING fts5(
  name,
  path,
  content='files',
  content_rowid='id'
);
```

通过触发器或显式同步保持与 `files` 一致。

### 6.4 表 `scan_jobs`

| 列 | 类型 | 说明 |
| --- | --- | --- |
| `id` | INTEGER PK | 自增 |
| `root_id` | INTEGER FK | 关联 `roots.id` |
| `started_at` | INTEGER | |
| `finished_at` | INTEGER NULL | |
| `status` | TEXT | `running` / `success` / `failed` / `cancelled` |
| `files_seen` | INTEGER | 本次扫描遍历到的文件数 |
| `error` | TEXT NULL | |

### 6.5 表 `settings`

简单 key-value，存放用户偏好（如默认排序、定时扫描间隔等）。

## 7. 索引根 (Root) 语义

### 7.1 自动 kind 检测

- `path` 以 `/Volumes/` 开头 → `nas`
- 其他 → `local`
- 用户在添加根时显式指定 `kind` 可以覆盖自动判断（例如某个外接 SSD 直挂在 `/Volumes` 但用户想当成 `local` 处理）。

### 7.2 状态机

```
available  ── 用户触发扫描 ──▶ scanning ── 成功 ─▶ available
                                       └─ 失败 ─▶ error
available  ── NAS 卷消失 ─────────────────────────▶ disconnected
disconnected ── 重连 + rescan 成功 ───────────────▶ available
任意状态  ── 用户删除根 ──▶ 从表中删除（同时清理 files）
```

### 7.3 NAS 不可用时的策略（关键约束）

- **绝不**因为 NAS 暂时不可见就 `DELETE FROM files WHERE root_id = ?`。
- 扫描启动前先检查根目录是否可访问；不可访问 → 标记 `disconnected`，**不动**已有 `files` 数据。
- 搜索仍然返回这些旧记录；前端在结果项上根据 root status 显示"不可用"标记。
- 用户尝试打开/Reveal 一个 root 状态为 `disconnected` 的文件 → 后端返回结构化错误，前端弹友好提示。

## 8. 扫描策略

### 8.1 MVP：手动全量扫描

- 用户对某个根点 "Rescan"，启动 `scan_jobs` 一条记录。
- 使用 `walkdir` 或 `jwalk` 递归遍历，生成 `(path, name, ext, size, mtime, is_dir)`。
- 写入策略采用 **upsert**（按 `(root_id, path)`），并把 `last_seen_scan` 更新为当前 job id。
- 扫描完成后，对同一 root 中 `last_seen_scan != current_job_id` 的记录执行删除（即"本次没看到的，说明已经被删除了"）。
  - 注意：仅在扫描**成功完成**时执行清理；失败 / 中断不清理，避免数据丢失。
- 进度通过 Tauri event `scan-progress` 回推前端，包含已扫描文件数。

### 8.2 V2：本地目录 FS Watch

- 使用 `notify` crate 对 `local` 根做 watch，增量更新 `files` 表。
- 仅对 `local` 根开启；`nas` 根禁用 watch（macOS FSEvents 在 SMB/NFS 上不靠谱）。

### 8.3 V2：NAS 目录定时扫描

- 在 `settings` 表中存储默认扫描间隔（如每 6 小时）。
- 后端启动一个调度器；只对 `nas` 且 `available` 的根触发。
- 必须支持手动取消和"暂停所有"。

## 9. Tauri Commands 契约

> 入参/出参用 TS 表达便于前端理解，对应 Rust 侧使用 `serde` 序列化。

```ts
type RootKind = "local" | "nas";
type RootStatus = "available" | "disconnected" | "scanning" | "error";

interface Root {
  id: number;
  path: string;
  kind: RootKind;
  status: RootStatus;
  lastScanAt: number | null;
  lastError: string | null;
}

interface FileHit {
  id: number;
  rootId: number;
  rootPath: string;
  rootStatus: RootStatus;
  path: string;
  name: string;
  parentDir: string;
  ext: string;
  size: number;
  mtime: number;
  isDir: boolean;
}

interface ScanStatus {
  rootId: number;
  status: "idle" | "running" | "success" | "failed";
  filesSeen: number;
  startedAt: number | null;
  finishedAt: number | null;
  error: string | null;
}

// Commands
add_index_root(path: string, kind?: RootKind): Promise<Root>;
list_index_roots(): Promise<Root[]>;
remove_index_root(rootId: number): Promise<void>;
start_scan(rootId: number): Promise<number>; // returns scan job id
rescan_root(rootId: number): Promise<number>;
get_scan_status(rootId: number): Promise<ScanStatus>;
search_files(query: string, limit?: number, offset?: number): Promise<FileHit[]>;
open_file(fileId: number): Promise<void>;
reveal_in_finder(fileId: number): Promise<void>;
```

## 10. 错误处理

- 所有命令统一返回 `Result<T, AppError>`，`AppError` 是结构化错误（`code` + `message`）。
- 前端 `invoke` 全部包 `try/catch`；错误进入状态栏或 toast，绝不让 React 树崩。
- 初次渲染**不依赖**任何后端命令成功；后端不可用时仍能看到完整骨架（标题 / 搜索框 / 空根列表 / 空结果 / 状态栏）。

## 11. 安全 & 权限

- macOS 第一次访问用户目录会触发 TCC（"完全磁盘访问 / 文件夹访问"）授权弹窗，由系统驱动。
- 应用本身不需要网络权限。
- 数据库文件保存在应用专属目录，不需要管理员权限。
- 不写任何用户数据到云端，不上传遥测（MVP 内不接入任何 telemetry）。

## 12. 依赖列表（建议）

Rust 侧（按需引入，不在 Stage 0 一次到位）：

- `tauri` (2.x) — 框架本体
- `tauri-plugin-opener` — 打开文件 / 在 Finder 中显示（已在模板中）
- `serde`, `serde_json` — 序列化
- `tokio` — 异步运行时（由 tauri 间接引入）
- `rusqlite`（启用 `bundled` + `fts5`）或 `sqlx`（feature `sqlite`）— SQLite + FTS5
- `walkdir` 或 `jwalk` — 递归遍历
- `notify`（仅 V2 watch 用）
- `thiserror` — 错误类型
- `anyhow` — 顶层错误聚合

前端侧：

- `react`, `react-dom`
- `@vitejs/plugin-react`
- `@tauri-apps/api`、`@tauri-apps/plugin-opener`
- `@tauri-apps/plugin-dialog`（目录选择器，必须用真实 dialog）
- 类型定义：`@types/react`, `@types/react-dom`

## 13. 构建与运行

- 前端开发：`npm run dev`
- 前端构建：`npm run build`（`tsc && vite build`）
- 桌面运行：`npm run tauri dev`（仅在 UI / Tauri 集成阶段使用）
- Rust 测试：`cd src-tauri && cargo test && cd ..`
- 桌面打包：`npm run tauri build`

## 14. 开放问题

- 数据库迁移：MVP 用最朴素的"启动时 CREATE IF NOT EXISTS"够用；V2 引入 `refinery` 或手写迁移表。
- 大量文件下 FTS5 重建策略：是否需要离线 batch 写而不是逐条 INSERT。
- macOS Sandbox / 公证：发布阶段再考虑，对 MVP 不影响。
