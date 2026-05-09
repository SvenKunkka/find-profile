# Find Profile · 开发计划 (Development Plan)

本计划把 MVP 拆成若干 **Stage**。每个 Stage 是一次小步快跑：明确输入、明确产出、明确验证命令、明确**不做**的事。严格遵循 CLAUDE.md 中"一次只做一个 Stage、不提前实现后面的内容"的规则。

## 0. 全局规则（来自 CLAUDE.md，再次强调）

- 一次只推进一个 Stage，不跳步。
- 后端阶段**禁止**跑 `npm run tauri dev`，除非明确要求。
- 后端阶段验证：`cd src-tauri && cargo test && cd ..`
- 前端阶段验证：`npm run build`
- 整体集成阶段验证：`npm run tauri dev`（仅手动验收时使用）
- 每完成一个 Stage：先跑该阶段的验证命令，再向用户报告变更文件，然后**等用户决定是否 commit**。
- 不在未要求的情况下重写大块代码，不删除已有可用测试。

## 1. Stage 0 — 现状盘点（无代码改动）

**目标**：确认仓库已包含 Tauri 模板骨架，列清当前能运行的最小路径。

**任务**：
- 阅读 `package.json`、`tsconfig.json`、`vite.config.ts`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`src/main.ts`、`src-tauri/src/lib.rs`。
- 在 `docs/` 目录维护本次产出的四份文档（已完成）。
- 不修改任何代码。

**产出**：
- `docs/product-spec.md`、`docs/architecture.md`、`docs/mvp-scope.md`、`docs/development-plan.md`。

**验证**：`ls docs`。

**完成判据**：四份文档存在且内容完整。

---

## 2. Stage 1 — 引入 React 前端骨架（前端阶段）

**目标**：把当前的 vanilla TS 模板替换为 React + TS 骨架，**仅渲染静态骨架**，不连后端。

**任务**：
1. `package.json` 增加依赖：`react`, `react-dom`, `@vitejs/plugin-react`, `@types/react`, `@types/react-dom`。
2. `vite.config.ts` 启用 React 插件。
3. `index.html` 改为只挂载 `<div id="root"></div>` 并加载 `/src/main.tsx`。
4. 新建 `src/main.tsx`：`createRoot(...).render(<App />)`。
5. 新建 `src/App.tsx`：渲染 5 个区块的静态骨架——
   - 顶部：标题 "Find Profile"
   - 搜索框（`<input>`，禁用提交，纯静态）
   - "Indexed Roots" 区（空列表占位 + "Add Root" 按钮，按钮 onClick 暂为 stub）
   - "Results" 区（空列表占位）
   - 底部状态栏（显示固定文字"Idle"）
6. `src/styles.css` 改成密集型工具风格（深色或中性灰，等宽字体可选）。
7. 删除模板里 `greet` 演示相关 DOM。

**MVP UI 硬性要求**：
- 标题、搜索框、Indexed Roots、Results、状态栏 5 个区块**必须出现**。
- 任何按钮必须是真实 `<button>`，且文字可见（不要图标-only）。
- 初次渲染**不依赖**任何 `invoke` 调用成功；即使后端缺失，骨架也要完整渲染。

**不做**：
- 不调用 Tauri command。
- 不接 dialog plugin（下一阶段做）。
- 不修改 Rust 后端。

**验证命令**：`npm run build`（必须通过 TS 编译 + Vite 构建）。

**完成判据**：`dist/` 产物生成；浏览器静态打开 `index.html` 至少能看到完整骨架。

---

## 3. Stage 2 — Rust 后端 DB & Roots 数据层（后端阶段）

**目标**：在 Rust 侧建立 SQLite 连接、迁移、`roots` 表，并实现 `add_index_root` / `list_index_roots` / `remove_index_root` 三个命令。**不做扫描，不做 FTS5。**

**任务**：
1. `src-tauri/Cargo.toml` 增加：`rusqlite`（features `bundled`, `fts5`）、`serde`/`serde_json`、`thiserror`、`anyhow`。
2. 新建 `src-tauri/src/db.rs`：打开 `<app-data>/find_profile.db` 连接，执行幂等的 schema 初始化（仅 `roots` + `settings` 表本阶段）。
3. 新建 `src-tauri/src/model.rs`：定义 `Root`、`RootKind`、`RootStatus`，含 `serde::Serialize`/`Deserialize`。
4. 新建 `src-tauri/src/roots.rs`：
   - `add_index_root(path, kind?) -> Root`，自动检测 kind（`/Volumes/` 前缀 → `nas`），允许显式覆盖。
   - `list_index_roots() -> Vec<Root>`。
   - `remove_index_root(root_id) -> ()`。
5. `src-tauri/src/lib.rs`：注册三个命令到 `invoke_handler`，启动时调用 db 初始化。
6. 编写单元测试：
   - 路径自动检测：`/Volumes/foo` → `nas`，`/Users/x` → `local`，显式覆盖生效。
   - 添加同一个路径两次应失败或返回原记录（取决于实现，明确一种语义并测试）。
   - 删除根后再列表应不包含该根。

**不做**：
- 不实现扫描、不建 `files` / `files_fts` / `scan_jobs` 表（下一阶段）。
- 不修改前端。

**验证命令**：`cd src-tauri && cargo test && cd ..`

**完成判据**：所有 cargo 测试通过；`lib.rs` 能编译并注册三个命令。

---

## 4. Stage 3 — Roots 管理前端接入（前端 + 集成）

**目标**：让 UI 真正可以"添加 / 列出 / 删除根"。

**任务**：
1. 安装并启用 `@tauri-apps/plugin-dialog`（前端 + Tauri capabilities 配置）。
2. 新建 `src/api/tauri.ts`：包装所有 invoke，集中 `try/catch`，把错误转换为类型化结果。
3. `RootsPanel` 组件：
   - 调用 `list_index_roots`，渲染列表（路径、kind 徽标、status 徽标、删除按钮）。
   - "Add Root" 按钮：用 `dialog.open({ directory: true })` 选目录，再调用 `add_index_root`。
   - 提供 kind 切换 UI（默认 = 自动检测，可显式指定 `local`/`nas`）。
   - 删除按钮调用 `remove_index_root`，乐观更新 + 失败回滚。
4. 状态栏显示根总数 / 当前选中的根（如果有）。
5. 错误一律走 `try/catch`，落到状态栏文本，不抛到 React。

**不做**：
- 不做扫描、不做搜索（下一阶段）。
- 不要把 dialog 替换为 `<input>`——必须用真实 Tauri dialog API。

**验证命令**：
- `npm run build`（前端构建过）
- 手动验收：`npm run tauri dev`，添加 `/Users/<me>/Documents` 和 `/Volumes/<some-volume>`，确认 kind 自动识别正确。

**完成判据**：UI 能新增、列出、删除根；重启应用后根仍在。

---

## 5. Stage 4 — Files 表 + FTS5 + 全量扫描（后端阶段）

**目标**：在后端实现"递归扫描 → 写入 files + files_fts → search_files 能查得到"的完整链路。

**任务**：
1. `db.rs` 增加表：`files`、`files_fts`（FTS5 外部内容表）、`scan_jobs`，加触发器或显式同步代码维护 `files_fts`。
2. 新建 `src-tauri/src/scanner.rs`：
   - `start_scan(root_id)`：异步任务，用 `walkdir` 递归遍历，批量 upsert（chunk 例如 1000 行一批）。
   - 每条 `files` 记录写入 `last_seen_scan = current_job_id`。
   - 扫描成功后清理 `last_seen_scan != current_job_id` 的孤儿记录。
   - 扫描失败 / 中断**不**清理（保护 NAS 旧数据语义）。
   - 通过 Tauri Event 周期性 emit `scan-progress`。
3. 新建 `src-tauri/src/search.rs`：`search_files(query, limit, offset)`，在 `files_fts` 上使用 FTS5 `MATCH`，并 join `roots` 返回 root status。
4. 新增命令：`start_scan`、`rescan_root`、`get_scan_status`、`search_files`。
5. NAS 不可达保护：扫描启动前 `metadata(root.path)` 失败 → 标记 `disconnected`，不开 job、不删 files。
6. 单元测试：
   - 在临时目录建若干文件，扫描后 search 命中预期文件名 / 路径片段。
   - 第二次扫描中删除一个真实文件后，对应记录被清理。
   - 模拟 root 路径不可达 → `files` 不被清空、root 状态变为 `disconnected`。

**不做**：
- 不做 watch、不做定时扫描（V2）。
- 不动前端。

**验证命令**：`cd src-tauri && cargo test && cd ..`

**完成判据**：所有 cargo 测试通过，包含上面三类用例。

---

## 6. Stage 5 — 搜索 / 结果列表 / 打开文件（前端 + 集成）

**目标**：把扫描和搜索接进 UI，让产品**可用**。

**任务**：
1. `SearchBar`：受控输入，debounce 150ms，调用 `search_files`。
2. `ResultsList`：虚拟列表（可用简单的窗口化或先纯渲染前 N 条），展示文件名、parentDir、size、mtime、root 标签 + status。
3. 双击行 → `open_file(fileId)`；右键 / 按钮 → `reveal_in_finder(fileId)`。
4. 每个根行新增 "Rescan" 按钮 → `rescan_root(rootId)`。
5. 监听 Tauri Event `scan-progress` / `scan-finished`，状态栏展示进度。
6. NAS 不可用结果行：视觉弱化（灰、加 "unavailable" 徽标），打开/Reveal 失败时弹友好 toast。

**不做**：
- 不做高级排序、不做复杂筛选器、不做导出。

**验证命令**：
- `npm run build`
- 手动验收：`npm run tauri dev`，按 MVP 验收清单（mvp-scope.md §4）逐项过一遍。

**完成判据**：MVP 验收清单全绿。

---

## 7. Stage 6（可选 / V2 起点）— 本地 watch + NAS 定时扫描

**目标**：在 MVP 验收完成后，平滑接入两类自动化扫描。

**任务**：
1. 引入 `notify`，对 `kind=local` 根启动 watch；事件 → 增量 upsert / 删除。
2. 实现一个简易调度器：周期性对所有 `kind=nas && status=available` 的根触发扫描。
3. `settings` 表加上"NAS 扫描间隔"，UI 提供修改。
4. 提供"暂停所有自动扫描"开关。

**不做**：
- 不对 `nas` 根开 watch。
- 不实现复杂的差分算法（依旧靠"全量遍历 + last_seen_scan"）。

**验证命令**：
- `cd src-tauri && cargo test && cd ..`
- `npm run build`
- 手动验收：`npm run tauri dev`。

**完成判据**：本地目录新建/删除文件能在数秒内反映到搜索结果；NAS 根按间隔自动扫描且可被取消。

---

## 8. 时间预算（指示性，不强制）

| Stage | 内容 | 估算 |
| --- | --- | --- |
| Stage 0 | 文档 | 0.5 天 |
| Stage 1 | React 骨架 | 0.5–1 天 |
| Stage 2 | DB + roots | 1–2 天 |
| Stage 3 | Roots UI 接入 | 1–2 天 |
| Stage 4 | Files + FTS5 + 扫描 | 2–4 天 |
| Stage 5 | 搜索 / 列表 / 打开 | 2–3 天 |
| **MVP 完成** | | **~7–13 天** |
| Stage 6 | watch + 定时 | 2–4 天（V2） |

## 9. 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| `walkdir` 在大型 NAS 上耗时长 → 用户感知卡 | 周期 emit 进度事件；保证扫描发生在后台任务，不阻塞 UI；首版可用 `jwalk` 提速。 |
| FTS5 在百万级 INSERT 时慢 | 批量事务（每 1000–5000 行一次 commit），扫描期间允许短暂落后。 |
| NAS 路径在扫描中途变得不可访问 | 捕获错误，标记 root 为 `error`，本次 job 标 `failed`，**不**做清理逻辑，保留旧数据。 |
| macOS TCC 权限拒绝 | 在 README / 状态栏给清晰提示，引导用户去"系统设置 → 隐私与安全性"开权限。 |
| 索引文件被多实例同时写入 | 通过单一应用实例（Tauri 默认）+ DB 写入串行化避免；不支持多实例。 |

## 10. 与 CLAUDE.md 的对应关系

- "Work one stage at a time" → §1–§7 严格分阶段。
- "Do not implement future stages early" → 每个 Stage 末尾的"不做"清单显式声明。
- "Backend-only stages must not run npm run tauri dev" → Stage 2、Stage 4 标注的验证命令不含 `tauri dev`。
- "Backend-only validation command: cd src-tauri && cargo test && cd .." → §3、§5 直接采用。
- "Frontend validation command: npm run build" → §2、§4、§6 直接采用。
- "Always run the requested validation command before saying the task is complete" → 每个 Stage 的"完成判据"都绑定到具体命令。
- Git 规则："Do not commit automatically unless explicitly asked" → 文档不规定 commit 时机，留给用户。
