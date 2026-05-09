import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type RootKind = "local" | "nas";
type RootStatus = "available" | "disconnected" | "scanning" | "error";
type SourceFilter = "all" | "local" | "nas";
type TypeFilter = "all" | "files" | "dirs";
type SortBy = "relevance" | "modified" | "size" | "name";

interface Root {
  id: number;
  path: string;
  kind: RootKind;
  enabled: boolean;
  status: RootStatus;
  scan_interval: string | null;
  last_scanned_at: string | null;
  created_at: string;
  updated_at: string;
}

interface FileHit {
  id: number;
  root_id: number;
  path: string;
  name: string;
  extension: string | null;
  size: number | null;
  modified_at: string | null;
  is_dir: boolean;
  source_kind: RootKind;
  is_accessible: boolean;
  indexed_at: string;
  root_status: RootStatus;
}

interface ScanStatus {
  root_id: number;
  job_id: number;
  status: "running" | "success" | "failed" | "cancelled";
  scanned_count: number;
  error_count: number;
  estimated_total_count: number | null;
  /** 0..=100, derived on the backend. 0 while the count phase is running. */
  progress_percent: number;
  current_path: string | null;
  started_at: string;
  finished_at: string | null;
  error_message: string | null;
}

interface Settings {
  search_default_limit: number;
  restore_watchers_on_startup: boolean;
  nas_scan_interval_minutes: number | null;
  show_hidden_files: boolean;
  index_files: boolean;
  index_folders: boolean;
  keep_disconnected_nas_index: boolean;
}

interface AppPaths {
  db_path: string;
  log_dir: string;
}

const SEARCH_DEBOUNCE_MS = 200;
const PAGE_SIZE = 50;
const SCAN_POLL_MS = 1000;

/**
 * Map a Tauri CommandError (or anything else thrown out of invoke()) to a
 * Chinese UI-facing string. The English `message` is intentionally dropped
 * so we never show raw technical text to the user; the original error is
 * always preserved in the matching `console.error` call for diagnostics.
 */
function translateError(err: unknown): string {
  const code =
    err && typeof err === "object"
      ? (err as { code?: string }).code
      : undefined;
  switch (code) {
    case "not_found":
      return "未找到对应记录";
    case "invalid_kind":
      return "根目录类型不合法";
    case "invalid_source":
      return "来源筛选不合法";
    case "invalid_type_filter":
      return "类型筛选不合法";
    case "invalid_sort":
      return "排序方式不合法";
    case "nas_unavailable":
      return "NAS 当前不可访问，请重新连接 NAS 后再次扫描";
    case "root_unavailable":
      return "根目录当前不可访问";
    case "db":
      return "数据库错误";
    case "io":
      return "输入 / 输出错误";
    case "unsupported":
      return "不支持的操作";
    case "internal":
      return "内部错误";
    case "path":
      return "路径解析失败";
    case "scan":
      return "扫描失败";
    case "tauri":
      return "系统错误";
    default:
      return "未知错误";
  }
}

function rootKindLabel(k: RootKind): string {
  return k === "local" ? "本地" : "NAS";
}

function rootStatusLabel(s: RootStatus): string {
  switch (s) {
    case "available":
      return "可用";
    case "disconnected":
      return "已断开";
    case "scanning":
      return "扫描中";
    case "error":
      return "错误";
  }
}

function formatSize(bytes: number | null, isDir: boolean): string {
  if (isDir) return "—";
  if (bytes == null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

function accessibilityLabel(hit: FileHit): string {
  if (hit.root_status === "disconnected" || hit.root_status === "error") {
    return "不可用";
  }
  return hit.is_accessible ? "是" : "否";
}

function accessibilityClass(hit: FileHit): string {
  if (hit.root_status === "disconnected" || hit.root_status === "error") {
    return "cell-access cell-access-unavailable";
  }
  return hit.is_accessible
    ? "cell-access cell-access-yes"
    : "cell-access cell-access-no";
}

function formatModified(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.valueOf())) return iso;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours()
  )}:${pad(d.getMinutes())}`;
}

function parseExtensionInput(input: string): string[] {
  // Split on comma OR whitespace; strip leading dots; lowercase; dedupe.
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of input.split(/[,\s]+/)) {
    const cleaned = raw.trim().replace(/^\./, "").toLowerCase();
    if (cleaned.length > 0 && !seen.has(cleaned)) {
      seen.add(cleaned);
      out.push(cleaned);
    }
  }
  return out;
}

function App() {
  const [roots, setRoots] = useState<Root[]>([]);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<FileHit[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [currentRoot, setCurrentRoot] = useState<Root | null>(null);
  const [currentPath, setCurrentPath] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);

  // Filters / sort / pagination.
  const [extensionInput, setExtensionInput] = useState("");
  const [sourceFilter, setSourceFilter] = useState<SourceFilter>("all");
  const [typeFilter, setTypeFilter] = useState<TypeFilter>("all");
  const [sortBy, setSortBy] = useState<SortBy>("relevance");
  const [page, setPage] = useState(0);

  const searchTimer = useRef<number | null>(null);

  // Settings modal.
  const [showSettings, setShowSettings] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [appPaths, setAppPaths] = useState<AppPaths | null>(null);
  const [settingsBusy, setSettingsBusy] = useState(false);

  // Per-root scan progress (only populated for roots with status=scanning).
  const [scanStatuses, setScanStatuses] = useState<Record<number, ScanStatus>>(
    {}
  );
  // True while the launch-time auto-scan is still finishing — surfaces the
  // status-bar hint "正在后台扫描已添加目录...".
  const [autoScanning, setAutoScanning] = useState(false);

  const hasScanningRoot = roots.some((r) => r.status === "scanning");

  const extensions = useMemo(
    () => parseExtensionInput(extensionInput),
    [extensionInput]
  );

  // Reset to page 0 whenever the *content* of the result list might change.
  useEffect(() => {
    setPage(0);
  }, [query, extensionInput, sourceFilter, typeFilter, sortBy]);

  /**
   * Returns true when the visible-to-the-user fields of two root snapshots
   * differ. Used by the polling effect to suppress identity-only setRoots
   * calls (which would re-render the whole results table just to re-paint
   * an unchanged sidebar). This is a hot path during scan because polling
   * fires every 1s.
   */
  function rootsChanged(a: Root[], b: Root[]): boolean {
    if (a.length !== b.length) return true;
    for (let i = 0; i < a.length; i += 1) {
      const x = a[i];
      const y = b[i];
      if (
        x.id !== y.id ||
        x.path !== y.path ||
        x.kind !== y.kind ||
        x.enabled !== y.enabled ||
        x.status !== y.status
      ) {
        return true;
      }
    }
    return false;
  }

  async function refreshRoots() {
    try {
      const next = await invoke<Root[]>("list_index_roots");
      setRoots((prev) => (rootsChanged(prev, next) ? next : prev));
      setCurrentRoot((prev) =>
        prev ? next.find((r) => r.id === prev.id) ?? null : prev
      );
    } catch (e) {
      console.error("list_index_roots failed", e);
      setError(`加载索引目录失败：${translateError(e)}`);
    }
  }

  useEffect(() => {
    (async () => {
      await refreshRoots();
      // Auto-scan every enabled root in the background. spawn_scan is non-
      // blocking; the global poll below picks up progress as workers run.
      try {
        const spawned = await invoke<number>("start_scan_all_roots");
        if (spawned > 0) {
          setAutoScanning(true);
        }
      } catch (e) {
        console.error("start_scan_all_roots failed", e);
        // Do not surface to UI; per-root errors land in scan_jobs and the
        // status badge already shows them as "错误" / "已断开".
      }
    })();
  }, []);

  // Global scan-progress poll. Active only while any root is scanning, so
  // searching against a quiet index doesn't pay the polling cost.
  useEffect(() => {
    if (!hasScanningRoot) {
      // Clean up percent badges as soon as no roots are scanning.
      if (Object.keys(scanStatuses).length > 0) {
        setScanStatuses({});
      }
      if (autoScanning) {
        setAutoScanning(false);
      }
      return;
    }
    const tick = window.setInterval(() => {
      (async () => {
        try {
          const next = await invoke<Root[]>("list_index_roots");
          // Avoid re-rendering the results table on every poll tick when
          // only the scan progress changed (progress lives in scanStatuses,
          // not roots).
          setRoots((prev) => (rootsChanged(prev, next) ? next : prev));
          const scanning = next.filter((r) => r.status === "scanning");
          // Fan out per-root status fetches in parallel.
          const entries = await Promise.all(
            scanning.map(async (r) => {
              try {
                const s = await invoke<ScanStatus | null>("get_scan_status", {
                  rootId: r.id,
                });
                return s && s.status === "running" ? ([r.id, s] as const) : null;
              } catch (err) {
                console.error("get_scan_status failed", err);
                return null;
              }
            })
          );
          const map: Record<number, ScanStatus> = {};
          for (const e of entries) {
            if (e) map[e[0]] = e[1];
          }
          setScanStatuses(map);
        } catch (err) {
          console.error("scan poll failed", err);
        }
      })();
    }, SCAN_POLL_MS);
    return () => window.clearInterval(tick);
  }, [hasScanningRoot]);

  // Debounced search. Re-runs on any filter / sort / page change.
  useEffect(() => {
    if (searchTimer.current !== null) {
      window.clearTimeout(searchTimer.current);
      searchTimer.current = null;
    }
    const trimmed = query.trim();
    if (trimmed.length === 0) {
      setResults([]);
      setSearched(false);
      return;
    }
    let cancelled = false;
    searchTimer.current = window.setTimeout(() => {
      (async () => {
        try {
          const hits = await invoke<FileHit[]>("search_files", {
            query: trimmed,
            limit: PAGE_SIZE,
            offset: page * PAGE_SIZE,
            extensions: extensions.length > 0 ? extensions : null,
            source: sourceFilter === "all" ? null : sourceFilter,
            typeFilter: typeFilter === "all" ? null : typeFilter,
            sort: sortBy,
          });
          if (!cancelled) {
            setResults(hits);
            setSearched(true);
          }
        } catch (e) {
          if (!cancelled) {
            console.error("search_files failed", e);
            setError(`搜索文件失败：${translateError(e)}`);
          }
        }
      })();
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      if (searchTimer.current !== null) {
        window.clearTimeout(searchTimer.current);
        searchTimer.current = null;
      }
    };
  }, [query, extensions, sourceFilter, typeFilter, sortBy, page]);

  async function handleAddRoot() {
    console.log("add root clicked");
    setError(null);

    let selected: string | string[] | null;
    try {
      console.log("opening directory dialog");
      selected = await open({ directory: true, multiple: false });
    } catch (error) {
      console.error("directory dialog failed", error);
      setError(`目录选择器失败：${translateError(error)}`);
      return;
    }

    console.log("selected root", selected);

    if (selected === null || selected === undefined) {
      return;
    }
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (typeof path !== "string" || path.length === 0) {
      return;
    }

    try {
      await invoke<Root>("add_index_root", { path, kind: null });
      await refreshRoots();
    } catch (e) {
      console.error("add_index_root failed", e);
      setError(`添加索引目录失败：${translateError(e)}`);
    }
  }

  async function handleCancel(rootId: number) {
    setError(null);
    try {
      await invoke<boolean>("cancel_scan", { rootId });
    } catch (e) {
      console.error("cancel_scan failed", e);
      setError(`取消扫描失败：${translateError(e)}`);
    }
  }

  async function handleScan(rootId: number) {
    setError(null);
    try {
      await invoke("start_scan", { rootId });
      await refreshRoots();
      // Global polling effect picks up status / percent updates from here.
    } catch (e) {
      console.error("start_scan failed", e);
      setError(`扫描目录失败：${translateError(e)}`);
    }
  }

  async function handleOpen(path: string) {
    setError(null);
    try {
      await invoke("open_file", { path });
    } catch (e) {
      console.error("open_file failed", e);
      setError(`打开文件失败：${translateError(e)}`);
    }
  }

  async function openSettings() {
    setShowSettings(true);
    if (settings === null || appPaths === null) {
      try {
        const [s, p] = await Promise.all([
          invoke<Settings>("get_settings"),
          invoke<AppPaths>("get_app_paths"),
        ]);
        setSettings(s);
        setAppPaths(p);
      } catch (e) {
        console.error("settings load failed", e);
        setError(`加载设置失败：${translateError(e)}`);
      }
    }
  }

  async function patchSettings(patch: Partial<Settings>) {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    setSettingsBusy(true);
    try {
      const saved = await invoke<Settings>("update_settings", {
        settings: next,
      });
      setSettings(saved);
    } catch (e) {
      console.error("update_settings failed", e);
      setError(`保存设置失败：${translateError(e)}`);
    } finally {
      setSettingsBusy(false);
    }
  }

  async function handleClearIndex() {
    if (
      !window.confirm(
        "确定要清理整个文件索引吗？\n\n根目录和设置会保留，但所有已索引的文件记录会被清除。在重新扫描之前搜索将返回空结果。"
      )
    ) {
      return;
    }
    if (!window.confirm("确认清理？此操作无法撤销。")) {
      return;
    }
    try {
      await invoke("clear_index");
      setError(null);
      setResults([]);
    } catch (e) {
      console.error("clear_index failed", e);
      setError(`清理索引失败：${translateError(e)}`);
    }
  }

  async function handleRebuildIndex() {
    if (
      !window.confirm(
        "确定要重建所有启用的根目录吗？\n\n当前索引会先被清空，然后对每个根目录启动新的扫描。已有搜索结果会立即消失，扫描完成后逐步恢复。"
      )
    ) {
      return;
    }
    if (
      !window.confirm("确认：清空当前索引并重新扫描所有启用的根目录？")
    ) {
      return;
    }
    try {
      await invoke("rebuild_index");
      setError(null);
      setResults([]);
      await refreshRoots();
    } catch (e) {
      console.error("rebuild_index failed", e);
      setError(`重建索引失败：${translateError(e)}`);
    }
  }

  async function handleOpenLogDir() {
    try {
      await invoke("open_log_dir");
    } catch (e) {
      console.error("open_log_dir failed", e);
      setError(`打开日志目录失败：${translateError(e)}`);
    }
  }

  async function handleReveal(path: string) {
    setError(null);
    try {
      await invoke("reveal_in_finder", { path });
    } catch (e) {
      console.error("reveal_in_finder failed", e);
      setError(`在 Finder 中显示文件失败：${translateError(e)}`);
    }
  }

  const trimmedQuery = query.trim();
  const hasQuery = trimmedQuery.length > 0;
  const isFirstPage = page === 0;
  const hasNextPage = results.length === PAGE_SIZE;

  const resultCountLabel = (() => {
    if (!hasQuery) return "";
    if (!searched) return "…";
    if (results.length === 0) return "共 0 条";
    return `第 ${page + 1} 页 · ${results.length} 条`;
  })();

  function sourceFilterLabel(s: SourceFilter): string {
    return { all: "全部", local: "本地", nas: "NAS" }[s];
  }
  function typeFilterLabel(t: TypeFilter): string {
    return { all: "全部", files: "文件", dirs: "文件夹" }[t];
  }

  return (
    <div className="app">
      <header className="app-header">
        <h1 className="app-title">Find Profile</h1>
        <input
          type="search"
          className="search-input"
          placeholder="搜索文件..."
          aria-label="搜索文件"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className="result-count" aria-live="polite">
          {resultCountLabel}
        </div>
        <button
          type="button"
          className="btn-settings"
          onClick={() => void openSettings()}
          aria-label="打开设置"
        >
          设置
        </button>
      </header>

      <main className="app-body">
        <aside className="roots-panel" aria-label="索引目录">
          <div className="panel-header">
            <h2 className="panel-title">索引目录</h2>
            <button
              type="button"
              className="btn-add-root"
              onClick={handleAddRoot}
            >
              + 添加目录
            </button>
          </div>
          {roots.length === 0 ? (
            <div className="panel-empty">还没有添加索引目录。</div>
          ) : (
            <ul className="roots-list">
              {roots.map((r) => (
                <li
                  key={r.id}
                  className={`root-item ${
                    currentRoot?.id === r.id ? "is-selected" : ""
                  }`}
                  onClick={() => setCurrentRoot(r)}
                >
                  <div className="root-path" title={r.path}>
                    {r.path}
                  </div>
                  <div className="root-meta">
                    <span className={`root-kind kind-${r.kind}`}>
                      {rootKindLabel(r.kind)}
                    </span>
                    <span className={`root-status status-${r.status}`}>
                      {rootStatusLabel(r.status)}
                    </span>
                    {r.status === "scanning" && (
                      <span
                        className="scan-progress"
                        title={
                          scanStatuses[r.id]?.current_path ?? undefined
                        }
                      >
                        {(() => {
                          const s = scanStatuses[r.id];
                          if (!s) return "扫描中...";
                          // First-ever scan of this root: no prior count to
                          // turn into a percentage. Surface live row count
                          // instead so the user still sees forward progress.
                          if (
                            s.estimated_total_count == null ||
                            s.estimated_total_count <= 0
                          ) {
                            return s.scanned_count > 0
                              ? `已扫描 ${s.scanned_count} 项`
                              : "扫描中...";
                          }
                          return `${s.progress_percent}%`;
                        })()}
                      </span>
                    )}
                    <button
                      type="button"
                      className={`btn-scan ${
                        r.status === "scanning" ? "btn-scan-cancel" : ""
                      }`}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (r.status === "scanning") {
                          void handleCancel(r.id);
                        } else {
                          void handleScan(r.id);
                        }
                      }}
                    >
                      {r.status === "scanning" ? "取消" : "扫描"}
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          <div className="roots-hints" role="note" aria-label="NAS 提示">
            <p>NAS 扫描速度取决于网络和 NAS 性能。</p>
            <p>NAS 断开后会保留旧索引，不会自动删除。</p>
          </div>
        </aside>

        <section className="results-panel" aria-label="搜索结果">
          <div className="panel-header">
            <h2 className="panel-title">搜索结果</h2>
          </div>

          <div className="filter-bar" role="toolbar" aria-label="搜索筛选">
            <label className="filter-field">
              <span>扩展名</span>
              <input
                type="text"
                value={extensionInput}
                onChange={(e) => setExtensionInput(e.target.value)}
                placeholder="pdf, txt, jpg"
                aria-label="按扩展名筛选"
              />
            </label>
            <label className="filter-field">
              <span>来源</span>
              <select
                value={sourceFilter}
                onChange={(e) =>
                  setSourceFilter(e.target.value as SourceFilter)
                }
                aria-label="按来源筛选"
              >
                <option value="all">全部</option>
                <option value="local">本地</option>
                <option value="nas">NAS</option>
              </select>
            </label>
            <label className="filter-field">
              <span>类型</span>
              <select
                value={typeFilter}
                onChange={(e) => setTypeFilter(e.target.value as TypeFilter)}
                aria-label="按类型筛选"
              >
                <option value="all">全部</option>
                <option value="files">文件</option>
                <option value="dirs">文件夹</option>
              </select>
            </label>
            <label className="filter-field">
              <span>排序</span>
              <select
                value={sortBy}
                onChange={(e) => setSortBy(e.target.value as SortBy)}
                aria-label="排序方式"
              >
                <option value="relevance">相关性</option>
                <option value="modified">修改时间</option>
                <option value="size">大小</option>
                <option value="name">名称</option>
              </select>
            </label>
          </div>

          {!hasQuery ? (
            <div className="panel-empty empty-help" role="status">
              <p>
                <strong>输入关键词</strong>搜索已索引文件（按名称或路径匹配）。
              </p>
              <p>
                在左侧面板添加目录并点击<em>扫描</em>后开始建立索引。
                文件名匹配优先于路径匹配。
              </p>
            </div>
          ) : !searched ? (
            <div className="panel-empty empty-help">搜索中…</div>
          ) : results.length === 0 ? (
            <div className="panel-empty empty-help" role="status">
              <p>
                没有找到匹配 <strong>"{trimmedQuery}"</strong> 的文件
                {extensions.length > 0
                  ? `（扩展名：${extensions.join("、")}）`
                  : ""}
                {sourceFilter !== "all"
                  ? ` · 来源：${sourceFilterLabel(sourceFilter)}`
                  : ""}
                {typeFilter !== "all"
                  ? ` · 类型：${typeFilterLabel(typeFilter)}`
                  : ""}
                。
              </p>
              <ul>
                <li>请尝试更短或不同的关键词。</li>
                <li>检查拼写是否正确。</li>
                <li>放宽上方的筛选条件（扩展名 / 来源 / 类型）。</li>
                <li>确认该文件所在的根目录已经在左侧添加并扫描完成。</li>
              </ul>
            </div>
          ) : (
            <>
              <div className="results-table-wrap">
                <table className="results-table">
                  <colgroup>
                    <col className="col-name" />
                    <col className="col-path" />
                    <col className="col-ext" />
                    <col className="col-size" />
                    <col className="col-modified" />
                    <col className="col-kind" />
                    <col className="col-source" />
                    <col className="col-access" />
                    <col className="col-actions" />
                  </colgroup>
                  <thead>
                    <tr>
                      <th>名称</th>
                      <th>路径</th>
                      <th>扩展名</th>
                      <th className="num">大小</th>
                      <th>修改时间</th>
                      <th>类型</th>
                      <th>来源</th>
                      <th>可访问</th>
                      <th aria-label="操作"></th>
                    </tr>
                  </thead>
                  <tbody>
                    {results.map((hit) => (
                      <tr
                        key={hit.id}
                        className={`result-row ${
                          currentPath === hit.path ? "is-selected" : ""
                        }`}
                        onClick={() => setCurrentPath(hit.path)}
                        onDoubleClick={() => void handleOpen(hit.path)}
                        title={hit.path}
                      >
                        <td className="cell-name">{hit.name}</td>
                        <td className="cell-path">{hit.path}</td>
                        <td>{hit.extension ?? "—"}</td>
                        <td className="num">
                          {formatSize(hit.size, hit.is_dir)}
                        </td>
                        <td>{formatModified(hit.modified_at)}</td>
                        <td>{hit.is_dir ? "文件夹" : "文件"}</td>
                        <td>
                          <span
                            className={`root-kind kind-${hit.source_kind}`}
                          >
                            {rootKindLabel(hit.source_kind)}
                          </span>
                        </td>
                        <td className={accessibilityClass(hit)}>
                          {accessibilityLabel(hit)}
                        </td>
                        <td className="cell-actions">
                          <button
                            type="button"
                            className="btn-reveal"
                            onClick={(e) => {
                              e.stopPropagation();
                              void handleReveal(hit.path);
                            }}
                          >
                            在 Finder 中显示
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="pagination" aria-label="分页">
                <button
                  type="button"
                  onClick={() => setPage((p) => Math.max(0, p - 1))}
                  disabled={isFirstPage}
                >
                  ← 上一页
                </button>
                <span className="pagination-page">
                  第 {page + 1} 页
                  {hasNextPage ? "" : " · 末页"}
                </span>
                <button
                  type="button"
                  onClick={() => setPage((p) => p + 1)}
                  disabled={!hasNextPage}
                >
                  下一页 →
                </button>
              </div>
            </>
          )}
        </section>
      </main>

      <footer className="status-bar" aria-label="状态">
        <div className="status-cell">
          <span className="status-label">状态</span>
          <span className="status-value">
            {autoScanning && hasScanningRoot
              ? "正在后台扫描已添加目录..."
              : `就绪 · ${roots.length} 个根目录${
                  hasQuery && searched
                    ? ` · 第 ${page + 1} 页：${results.length} 条`
                    : ""
                }`}
          </span>
        </div>
        <div className="status-cell">
          <span className="status-label">根目录</span>
          <span className="status-value" title={currentRoot?.path ?? undefined}>
            {currentRoot?.path ?? "—"}
          </span>
        </div>
        <div className="status-cell">
          <span className="status-label">路径</span>
          <span className="status-value" title={currentPath ?? undefined}>
            {currentPath ?? "—"}
          </span>
        </div>
        <div
          className={`status-cell status-cell-error ${
            error ? "is-active" : ""
          }`}
          role={error ? "alert" : undefined}
        >
          <span className="status-label">错误</span>
          <span className="status-value" title={error ?? undefined}>
            {error ?? "—"}
          </span>
          {error && (
            <button
              type="button"
              className="status-dismiss"
              onClick={() => setError(null)}
              aria-label="关闭错误提示"
            >
              ×
            </button>
          )}
        </div>
      </footer>

      {showSettings && (
        <div
          className="modal-overlay"
          onClick={() => setShowSettings(false)}
          role="presentation"
        >
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-label="设置"
            onClick={(e) => e.stopPropagation()}
          >
            <header className="modal-header">
              <h2>设置</h2>
              <button
                type="button"
                className="modal-close"
                onClick={() => setShowSettings(false)}
                aria-label="关闭设置"
              >
                ×
              </button>
            </header>
            <div className="modal-body">
              {!settings ? (
                <p className="modal-loading">加载中…</p>
              ) : (
                <>
                  <section className="settings-section">
                    <h3>搜索</h3>
                    <label className="settings-row">
                      <span>每页默认数量</span>
                      <input
                        type="number"
                        min={1}
                        max={1000}
                        value={settings.search_default_limit}
                        onChange={(e) =>
                          void patchSettings({
                            search_default_limit:
                              Number(e.target.value) || 1,
                          })
                        }
                      />
                    </label>
                  </section>

                  <section className="settings-section">
                    <h3>索引选项</h3>
                    <label className="settings-row">
                      <input
                        type="checkbox"
                        checked={settings.index_files}
                        onChange={(e) =>
                          void patchSettings({ index_files: e.target.checked })
                        }
                      />
                      <span>索引文件</span>
                    </label>
                    <label className="settings-row">
                      <input
                        type="checkbox"
                        checked={settings.index_folders}
                        onChange={(e) =>
                          void patchSettings({
                            index_folders: e.target.checked,
                          })
                        }
                      />
                      <span>索引文件夹</span>
                    </label>
                    <label className="settings-row">
                      <input
                        type="checkbox"
                        checked={settings.show_hidden_files}
                        onChange={(e) =>
                          void patchSettings({
                            show_hidden_files: e.target.checked,
                          })
                        }
                      />
                      <span>
                        显示隐藏文件
                        <em className="settings-hint">
                          {" "}
                          （文件名以 <code>.</code> 开头的文件）
                        </em>
                      </span>
                    </label>
                  </section>

                  <section className="settings-section">
                    <h3>监听与 NAS</h3>
                    <label className="settings-row">
                      <input
                        type="checkbox"
                        checked={settings.restore_watchers_on_startup}
                        onChange={(e) =>
                          void patchSettings({
                            restore_watchers_on_startup: e.target.checked,
                          })
                        }
                      />
                      <span>启动时恢复本地目录监听</span>
                    </label>
                    <label className="settings-row">
                      <span>NAS 扫描间隔（分钟）</span>
                      <input
                        type="number"
                        min={0}
                        placeholder="关闭"
                        value={settings.nas_scan_interval_minutes ?? ""}
                        onChange={(e) => {
                          const raw = e.target.value;
                          const v = raw.length === 0 ? null : Number(raw);
                          void patchSettings({
                            nas_scan_interval_minutes:
                              v === null || Number.isNaN(v) || v < 0 ? null : v,
                          });
                        }}
                      />
                    </label>
                    <label className="settings-row settings-row-disabled">
                      <input
                        type="checkbox"
                        checked={settings.keep_disconnected_nas_index}
                        disabled
                        readOnly
                      />
                      <span>
                        NAS 断开时保留索引
                        <em className="settings-hint">
                          {" "}
                          （始终开启 - 见 docs/architecture.md）
                        </em>
                      </span>
                    </label>
                  </section>

                  <section className="settings-section">
                    <h3>存储</h3>
                    <div className="settings-row">
                      <span>数据库</span>
                      <code className="settings-path" title={appPaths?.db_path}>
                        {appPaths?.db_path ?? "—"}
                      </code>
                    </div>
                    <div className="settings-row">
                      <span>日志</span>
                      <code
                        className="settings-path"
                        title={appPaths?.log_dir}
                      >
                        {appPaths?.log_dir ?? "—"}
                      </code>
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={() => void handleOpenLogDir()}
                      >
                        打开
                      </button>
                    </div>
                  </section>

                  <section className="settings-section settings-section-danger">
                    <h3>危险操作</h3>
                    <p className="settings-danger-note">
                      这些操作会影响所有根目录，且无法撤销。每次点击都会请求两次确认。
                    </p>
                    <div className="settings-row">
                      <button
                        type="button"
                        className="btn-danger"
                        onClick={() => void handleClearIndex()}
                      >
                        清理索引
                      </button>
                      <button
                        type="button"
                        className="btn-danger"
                        onClick={() => void handleRebuildIndex()}
                      >
                        重建全部索引
                      </button>
                    </div>
                  </section>

                  {settingsBusy && (
                    <p className="modal-loading">保存中…</p>
                  )}
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
