# Find Profile · 性能基线与测量方法 (Performance)

记录 Find Profile 在 100k 文件量级下的目标、设计点、以及如何在本机复现测量。MVP 不接 CI 性能门禁；这份文档是**人手 dev 验证清单 + 数字感觉的锚点**。

## 1. 目标 (MVP)

| 项 | 目标 | 说明 |
| --- | --- | --- |
| 100k 索引 | 稳定完成不 crash | 单 root，10 万文件递归扫描成功率 100% |
| 搜索响应 | < 100ms | 100k 行 FTS5 上单关键词检索 + 默认 BM25 排序 |
| UI 冻结 | 0 | 扫描进行中点击搜索/列表 root/打开文件 应当在百毫秒级响应 |
| 扫描中搜索 | 可用 | 已写入索引的行立刻可被搜索到 |
| 取消响应 | < 1s | 点击 Cancel 后 worker 在 1 秒内停止（按当前 walker 进入下一个 entry 计） |

## 2. 关键设计

为了让 100k 索引 + UI 不卡这两件事同时成立，扫描器做了几条结构性约束：

1. **批量事务。** `BATCH_SIZE = 500` 行做一次 `BEGIN…COMMIT`，固定事务开销摊薄到 500 个 upsert 上。源代码：`src-tauri/src/scanner/mod.rs::scan_cancellable` 的 `flush_locked`。
2. **锁按批获取/释放。** scanner 不再 `&mut Connection` 一握到底，而是接 `Arc<Mutex<Connection>>` 在每个 batch 边界 `lock` → flush → drop。两批之间的 50–200ms 走 walker（不持锁），这段时间 search_files / list_index_roots / 文件 watcher 都能见缝插针进 DB。
3. **节流进度更新。** scan_jobs 进度 + on_progress 回调按 `每 2500 条 OR 每 500ms` 中较先者触发，避免 200 多次小 UPDATE 抢锁。源代码：`PROGRESS_DB_INTERVAL` / `PROGRESS_TIME_MS`。
4. **取消是 cooperative。** `Arc<AtomicBool>` 注册到 `ScanCancellations`，worker 每个 walker 迭代顶端做一次 `Relaxed` load。`cancel_scan` 命令把它翻成 true，worker 在下一条 entry 处出循环、做 final flush、写 `scan_jobs.status='cancelled'` + root 回 `available`。**已写入索引绝不 rollback。**
5. **错误聚合。** walker / extract_metadata 失败 → `error_count++`，扫描结束在 `scan_jobs.error_message` 写一条汇总（如 `scan completed: 100000 indexed, 12 skipped`），不打多条 stderr 噪音。
6. **FTS5 BM25 加权。** 名字列权重 10×，路径列权重 1×。100k 行下检索"foo"通常在 5–10ms（取决于词的 IDF 分布）。

## 3. 复现：构造 100k 测试树

```bash
mkdir -p /tmp/find_profile_perf/root
cd /tmp/find_profile_perf/root

# 扁平 1 万 + 10 个子目录 × 1 万 = 11 万文件
for i in $(seq 1 10000); do : > "flat_$i.txt"; done
for d in $(seq 1 10); do
  mkdir -p "subdir_$d"
  for i in $(seq 1 10000); do : > "subdir_$d/note_${d}_$i.md"; done
done

# 验证总文件数
find /tmp/find_profile_perf/root -type f | wc -l
```

如果只想压一个 root 内的字符多样性，可以混入若干扩展名（pdf、jpg、log 等空文件），让扩展名过滤的真实分布更像生产。

## 4. 测量步骤

### 4.1 扫描吞吐

1. `npm run tauri dev` 启动 app。
2. 添加 root 指向 `/tmp/find_profile_perf/root`。
3. 点击 **Scan**，开始时记下挂钟时间 `t0`。
4. 等左侧 root 状态从 `scanning` 回到 `available`，记下 `t1`。
5. 看 `Status` 段的 hits 数 / scan_jobs 表的 scanned_count 是否 ≈ 11 万。

合格基线：单 root、110k 文件、APFS、SSD、Apple Silicon —— 60–150 秒之间属正常区间。慢于 5 分钟，先看 scan_jobs.error_count（FS 错误激增意味着不是扫描器瓶颈）。

或者跑 SQLite 直接查：

```sql
-- 在打开的 app 数据库里
SELECT id, scanned_count, error_count, started_at, finished_at FROM scan_jobs ORDER BY id DESC LIMIT 1;
```

### 4.2 搜索响应（< 100ms 目标）

1. 扫描完成、root 状态 available。
2. 在搜索框输入一个常见词（例如 `note`），等 200ms debounce 触发。
3. DevTools Performance / Network 面板看 `search_files` invoke 的 round-trip 时间。

或者用 Rust 微基准（推荐做粗略的"百万行下界限"测量，需要时再加）：

```rust
// tests/perf.rs (示意，不在 MVP 测试套件)
let start = std::time::Instant::now();
let hits = search_files(&conn, "note", 200, 0)?;
let dt = start.elapsed();
println!("{} hits in {:?}", hits.len(), dt);
```

合格基线：100k 行下，prefix-token 单词检索 < 50ms（典型 5–15ms）；带过滤（扩展名 + source）+ 排序仍应 < 100ms。

### 4.3 扫描中搜索

1. 在一个大目录上点 **Scan** —— 状态变 `scanning`。
2. 在搜索框输入关键词。
3. 应当在 200ms debounce + 一次 batch 锁释放窗口内（≤ 500ms 总）返回结果。
4. 多次输入不同关键词，结果计数随扫描进度增长。

合格基线：扫描进行中，搜索 round-trip 不应超过 1s。如果 UI 长时间转圈，说明 lock 持有时间过长 —— 检查 `BATCH_SIZE` 与 `flush_locked` 是否被绕过。

### 4.4 UI 冻结排查

扫描进行中持续点击：
- 左侧 root 列表（应即时高亮）
- `+ Add Root` 按钮（应能弹出目录选择器，无延迟）
- 双击已有结果行（应能调 open_file）

任何一项 > 1s 无响应，开 DevTools Performance 录一段，最大概率是 invoke 在排 SQLite mutex 队列。

### 4.5 取消响应

1. 在大目录上点 **Scan**。
2. 等扫描进行 10–20 秒（已写入若干批）。
3. 点 **Cancel**。
4. 左侧 root 状态 `scanning` → `available`，过程应 < 1s。
5. 看 scan_jobs 最近一条：`status='cancelled'`、`error_message` 含 "cancelled by user"、`scanned_count > 0`。
6. 在搜索框搜索某个被部分扫描到的文件名 —— 应能命中（旧索引保留）。
7. 再点 **Scan**，应能正常恢复并把剩下的文件补齐。

## 5. 已知边界 / 不做事项

- 没有 connection pool。所有 DB 访问串行在单 `Mutex<Connection>` 上。lock-per-batch 已经把扫描期间锁占用打到很低（每批 ~ms 级），但极重负载下 search 仍可能短排队。
- 没有 `notify-debouncer`。watcher 触发的连续事件会触发重复 upsert（幂等，无正确性问题，写放大）。
- 没有 query cache。每次搜索都是新的 prepare + bm25 计算。MVP 数据量级可接受。
- 没有 CI 性能门禁。这份文档是 dev 自检清单，不接到测试套件里。当前 `cargo test` 套件覆盖功能正确性 + 取消语义，不覆盖 100k 性能数字。

## 6. 2026-05 性能优化清单

第一版扫描器在大目录（百万级，NAS 上更明显）上有两个主要瓶颈：(1) 任何时候点 Scan/启动 app 都跑一遍 Phase-1 pre-walk 计数，等于把整棵树走两遍；(2) 启动时同时对所有 root 触发并发扫描，导致磁盘和 SQLite 互锁互相拖累。下面这一组改动同时落地，目标是把"重复扫描"和"启动扫描"两个最常见路径的耗时显著降下来。

| 优化 | 影响 | 源代码 |
| --- | --- | --- |
| **SQLite PRAGMA 调优** | WAL + synchronous=NORMAL → 扫描期间搜索几乎零等待；temp_store=MEMORY、cache_size=-65536 (64 MB) → FTS posting list 构建不再压本地磁盘 | `src-tauri/src/db/mod.rs::configure` |
| **删除 Phase-1 pre-walk 计数** | 直接砍掉一整轮 readdir 流量。NAS 上肉眼可感知（少一轮往返） | `src-tauri/src/scanner/mod.rs::scan_cancellable` |
| **HashMap 增量扫描** | 启动时把当前 root 的索引读进 `HashMap<path, FileSnapshot>`，walker 边走边 `remove()`：metadata 全等 → 跳过 upsert（典型 no-op rescan 提速 ≥ 20×）；剩在 map 里的就是磁盘上消失了的，一次性 `mark_files_inaccessible` 标记不可用而不删行 | `scan_cancellable` + `db::load_root_file_index` + `db::mark_files_inaccessible` |
| **顺序队列自动扫描** | `start_scan_all_roots` / `rebuild_index` 不再 N 个 root N 个线程，而是入一个队列、单 worker 串行处理。NAS 慢扫不会再卡 local 快扫的 mutex/磁盘带宽 | `src-tauri/src/commands/handlers.rs::scan_all_roots_inner` |
| **`try_register` 防双扫** | 用户连点 Scan 或自动扫描和手动扫描相撞时，第二次进入 `spawn_scan` 直接 no-op，不再把第一个 worker 的 cancel token 顶掉变成孤儿 | `src-tauri/src/scanner/cancel.rs::try_register` |
| **前端 roots 比较** | 1 秒一次的扫描进度轮询不再在每次 tick 都触发整表重渲染：只有 `id/path/kind/enabled/status` 真正变化才 `setRoots`，进度 % 走单独的 `scanStatuses` state | `src/App.tsx::rootsChanged` |
| **首次扫描进度文案** | 第一次扫一个 root 时没有"上一次的 count"可以推 %，前端改为显示 `已扫描 N 项` 而不是干转圈 | `src/App.tsx` 的 `scan-progress` 渲染 |

### 6.1 关键不变量（增量扫描下仍然成立）

- **取消绝不破坏索引：** `scan_cancellable` 在 cancelled 分支跳过 `mark_files_inaccessible`。半途 cancel 后旧数据原样保留。
- **NAS 断开绝不破坏索引：** preflight 失败的 root 直接 `Failed → Disconnected`，根本不走入增量循环，`mark_files_inaccessible` 不会触达。
- **mark-missing 不删行：** 缺失文件只把 `is_accessible = 0` 置位 + 刷 `indexed_at`，FTS 行保留，搜索能继续命中（前端会把它们渲染为 `不可用`）。

### 6.2 这些优化的覆盖测试

| 行为 | 测试 |
| --- | --- |
| 重扫不变文件不再 upsert | `scanner::tests::unchanged_rescan_does_not_bump_indexed_at` |
| 删除文件后重扫被 mark inaccessible（不删行） | `scanner::tests::rescan_marks_deleted_files_inaccessible_and_keeps_their_row` |
| cancel 路径不 mark missing | `scanner::tests::cancelled_rescan_does_not_mark_missing` |
| NAS preflight 失败不 mark missing | `scanner::tests::nas_disconnected_preflight_does_not_mark_missing` |
| 首次扫描 estimated_total = None / 重扫为 Some(prev) | `commands::tests::scan_populates_estimated_total_count_on_rescan` |
| `try_register` 防重入 | `scanner::cancel::tests::try_register_returns_none_when_already_active` |

## 7. 出问题先看哪里

| 症状 | 第一手怀疑 |
| --- | --- |
| 100k 扫描卡死 | 看 scan_jobs.error_count；如果 walker 在某个目录上 retry，可能权限 / FS 错误循环 |
| UI 冻结 | DevTools Performance 录一段；找最长的 invoke 等待。98% 是某个长 lock hold —— 看 scanner 的 batch / flush 是否被改坏 |
| 搜索慢 | 用 EXPLAIN QUERY PLAN 看 SQL；确认 fts5 MATCH 走 fts 表而不是全表扫；确认 ORDER BY bm25 没被改成全表排序 |
| 取消不响应 | 检查 worker 线程是否真的拿到了 cancel token；`ScanCancellations::try_register` 在每次 spawn_scan 入口都调；CMD `cancel_scan` 返回 `false` 说明 token 已被 clear（scan 已完成） |
| 重扫还是慢 | 看 scan_jobs 最近一条的 estimated_total_count 和 scanned_count 比例。如果接近 1:1 但耗时仍长，多半是 metadata 真的变了（modified_at 漂移）—— 用 `stat` 或 `find -newer` 直接对照磁盘上的文件 |
| 启动后看到多个 root 同时 scanning | 不应再出现：自动扫描进了单 worker 队列。如果出现，多半是同时手点了 Scan 按钮（手动 `start_scan` 仍会单独 spawn）—— 这是预期行为 |
