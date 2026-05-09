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

interface ScanJob {
  id: number;
  root_id: number;
  status: "running" | "success" | "failed" | "cancelled";
  scanned_count: number;
  error_count: number;
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

interface CommandError {
  code: string;
  message: string;
}

const SEARCH_DEBOUNCE_MS = 200;
const PAGE_SIZE = 50;
const SCAN_POLL_MS = 1500;
const SCAN_POLL_TIMEOUT_MS = 5 * 60 * 1000;

function formatError(err: unknown): string {
  if (err == null) return "unknown error";
  if (typeof err === "string") return err;
  if (typeof err === "object") {
    const maybe = err as Partial<CommandError>;
    if (typeof maybe.message === "string" && maybe.message.length > 0) {
      return maybe.message;
    }
    try {
      return JSON.stringify(err);
    } catch {
      return Object.prototype.toString.call(err);
    }
  }
  return String(err);
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
    return "unavailable";
  }
  return hit.is_accessible ? "yes" : "no";
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

  const extensions = useMemo(
    () => parseExtensionInput(extensionInput),
    [extensionInput]
  );

  // Reset to page 0 whenever the *content* of the result list might change.
  useEffect(() => {
    setPage(0);
  }, [query, extensionInput, sourceFilter, typeFilter, sortBy]);

  async function refreshRoots() {
    try {
      const next = await invoke<Root[]>("list_index_roots");
      setRoots(next);
      setCurrentRoot((prev) =>
        prev ? next.find((r) => r.id === prev.id) ?? null : prev
      );
    } catch (e) {
      console.error("list_index_roots failed", e);
      setError(`Failed to load roots: ${formatError(e)}`);
    }
  }

  useEffect(() => {
    void refreshRoots();
  }, []);

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
            setError(`Search failed: ${formatError(e)}`);
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
      setError(`Directory picker failed: ${formatError(error)}`);
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
      setError(`Failed to add root: ${formatError(e)}`);
    }
  }

  async function handleCancel(rootId: number) {
    setError(null);
    try {
      await invoke<boolean>("cancel_scan", { rootId });
    } catch (e) {
      console.error("cancel_scan failed", e);
      setError(`Failed to cancel scan: ${formatError(e)}`);
    }
  }

  async function handleScan(rootId: number) {
    setError(null);
    try {
      await invoke("start_scan", { rootId });
      await refreshRoots();

      const start = Date.now();
      const tick = window.setInterval(() => {
        (async () => {
          try {
            await refreshRoots();
            const job = await invoke<ScanJob | null>("get_scan_status", {
              rootId,
            });
            const stillRunning = job != null && job.status === "running";
            if (!stillRunning || Date.now() - start > SCAN_POLL_TIMEOUT_MS) {
              window.clearInterval(tick);
            }
          } catch (err) {
            console.error("get_scan_status failed", err);
            window.clearInterval(tick);
          }
        })();
      }, SCAN_POLL_MS);
    } catch (e) {
      console.error("start_scan failed", e);
      setError(`Failed to start scan: ${formatError(e)}`);
    }
  }

  async function handleOpen(path: string) {
    setError(null);
    try {
      await invoke("open_file", { path });
    } catch (e) {
      console.error("open_file failed", e);
      setError(`Failed to open file: ${formatError(e)}`);
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
        setError(`Failed to load settings: ${formatError(e)}`);
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
      setError(`Failed to save settings: ${formatError(e)}`);
    } finally {
      setSettingsBusy(false);
    }
  }

  async function handleClearIndex() {
    if (
      !window.confirm(
        "Clear the entire file index?\n\nRoots and settings are kept, but every cached file row is removed. Search will return nothing until you rescan."
      )
    ) {
      return;
    }
    if (
      !window.confirm(
        "Are you absolutely sure? This action cannot be undone."
      )
    ) {
      return;
    }
    try {
      await invoke("clear_index");
      setError(null);
      setResults([]);
    } catch (e) {
      console.error("clear_index failed", e);
      setError(`Failed to clear index: ${formatError(e)}`);
    }
  }

  async function handleRebuildIndex() {
    if (
      !window.confirm(
        "Rebuild every enabled root?\n\nThe current index is wiped first, then a scan is started for each root. Existing search results disappear immediately and reappear as scans complete."
      )
    ) {
      return;
    }
    if (
      !window.confirm(
        "Confirm: clear current index and rescan every enabled root?"
      )
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
      setError(`Failed to rebuild index: ${formatError(e)}`);
    }
  }

  async function handleOpenLogDir() {
    try {
      await invoke("open_log_dir");
    } catch (e) {
      console.error("open_log_dir failed", e);
      setError(`Failed to open log directory: ${formatError(e)}`);
    }
  }

  async function handleReveal(path: string) {
    setError(null);
    try {
      await invoke("reveal_in_finder", { path });
    } catch (e) {
      console.error("reveal_in_finder failed", e);
      setError(`Failed to reveal in Finder: ${formatError(e)}`);
    }
  }

  const trimmedQuery = query.trim();
  const hasQuery = trimmedQuery.length > 0;
  const isFirstPage = page === 0;
  const hasNextPage = results.length === PAGE_SIZE;

  const resultCountLabel = (() => {
    if (!hasQuery) return "";
    if (!searched) return "…";
    if (results.length === 0) return "0 results";
    return `${results.length} on page ${page + 1}`;
  })();

  return (
    <div className="app">
      <header className="app-header">
        <h1 className="app-title">Find Profile</h1>
        <input
          type="search"
          className="search-input"
          placeholder="Search files..."
          aria-label="Search files"
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
          aria-label="Open settings"
        >
          Settings
        </button>
      </header>

      <main className="app-body">
        <aside className="roots-panel" aria-label="Indexed Roots">
          <div className="panel-header">
            <h2 className="panel-title">Indexed Roots</h2>
            <button
              type="button"
              className="btn-add-root"
              onClick={handleAddRoot}
            >
              + Add Root
            </button>
          </div>
          {roots.length === 0 ? (
            <div className="panel-empty">No roots yet.</div>
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
                    <span className={`root-kind kind-${r.kind}`}>{r.kind}</span>
                    <span className={`root-status status-${r.status}`}>
                      {r.status}
                    </span>
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
                      {r.status === "scanning" ? "Cancel" : "Scan"}
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          <div className="roots-hints" role="note" aria-label="NAS notes">
            <p>NAS scanning speed depends on network and NAS performance.</p>
            <p>Disconnected NAS roots keep their old index.</p>
          </div>
        </aside>

        <section className="results-panel" aria-label="Results">
          <div className="panel-header">
            <h2 className="panel-title">Results</h2>
          </div>

          <div className="filter-bar" role="toolbar" aria-label="Search filters">
            <label className="filter-field">
              <span>Ext</span>
              <input
                type="text"
                value={extensionInput}
                onChange={(e) => setExtensionInput(e.target.value)}
                placeholder="pdf, txt, jpg"
                aria-label="Filter by extension"
              />
            </label>
            <label className="filter-field">
              <span>Source</span>
              <select
                value={sourceFilter}
                onChange={(e) =>
                  setSourceFilter(e.target.value as SourceFilter)
                }
                aria-label="Filter by source"
              >
                <option value="all">All</option>
                <option value="local">Local</option>
                <option value="nas">NAS</option>
              </select>
            </label>
            <label className="filter-field">
              <span>Type</span>
              <select
                value={typeFilter}
                onChange={(e) => setTypeFilter(e.target.value as TypeFilter)}
                aria-label="Filter by type"
              >
                <option value="all">All</option>
                <option value="files">Files</option>
                <option value="dirs">Folders</option>
              </select>
            </label>
            <label className="filter-field">
              <span>Sort</span>
              <select
                value={sortBy}
                onChange={(e) => setSortBy(e.target.value as SortBy)}
                aria-label="Sort by"
              >
                <option value="relevance">Relevance</option>
                <option value="modified">Modified</option>
                <option value="size">Size</option>
                <option value="name">Name</option>
              </select>
            </label>
          </div>

          {!hasQuery ? (
            <div className="panel-empty empty-help" role="status">
              <p>
                <strong>Type to search</strong> indexed files by name or path.
              </p>
              <p>
                Add a root in the left panel and click <em>Scan</em> to populate
                the index. Filenames rank above path matches.
              </p>
            </div>
          ) : !searched ? (
            <div className="panel-empty empty-help">Searching…</div>
          ) : results.length === 0 ? (
            <div className="panel-empty empty-help" role="status">
              <p>
                No matches for <strong>"{trimmedQuery}"</strong>
                {extensions.length > 0
                  ? ` (extensions: ${extensions.join(", ")})`
                  : ""}
                {sourceFilter !== "all" ? ` · source: ${sourceFilter}` : ""}
                {typeFilter !== "all" ? ` · type: ${typeFilter}` : ""}
                .
              </p>
              <ul>
                <li>Try a shorter or different keyword.</li>
                <li>Check the spelling.</li>
                <li>Loosen the filters above (Ext / Source / Type).</li>
                <li>
                  Confirm the file's root is added and scanned in the left
                  panel.
                </li>
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
                      <th>Name</th>
                      <th>Path</th>
                      <th>Extension</th>
                      <th className="num">Size</th>
                      <th>Modified</th>
                      <th>Kind</th>
                      <th>Source</th>
                      <th>Accessible</th>
                      <th aria-label="Actions"></th>
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
                        <td>{hit.is_dir ? "folder" : "file"}</td>
                        <td>
                          <span
                            className={`root-kind kind-${hit.source_kind}`}
                          >
                            {hit.source_kind}
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
                            Reveal
                          </button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="pagination" aria-label="Pagination">
                <button
                  type="button"
                  onClick={() => setPage((p) => Math.max(0, p - 1))}
                  disabled={isFirstPage}
                >
                  ← Prev
                </button>
                <span className="pagination-page">
                  Page {page + 1}
                  {hasNextPage ? "" : " · end"}
                </span>
                <button
                  type="button"
                  onClick={() => setPage((p) => p + 1)}
                  disabled={!hasNextPage}
                >
                  Next →
                </button>
              </div>
            </>
          )}
        </section>
      </main>

      <footer className="status-bar" aria-label="Status">
        <div className="status-cell">
          <span className="status-label">Status</span>
          <span className="status-value">
            Ready · {roots.length} root{roots.length === 1 ? "" : "s"}
            {hasQuery && searched
              ? ` · page ${page + 1}: ${results.length} hit${
                  results.length === 1 ? "" : "s"
                }`
              : ""}
          </span>
        </div>
        <div className="status-cell">
          <span className="status-label">Root</span>
          <span className="status-value" title={currentRoot?.path ?? undefined}>
            {currentRoot?.path ?? "—"}
          </span>
        </div>
        <div className="status-cell">
          <span className="status-label">Path</span>
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
          <span className="status-label">Error</span>
          <span className="status-value" title={error ?? undefined}>
            {error ?? "—"}
          </span>
          {error && (
            <button
              type="button"
              className="status-dismiss"
              onClick={() => setError(null)}
              aria-label="Dismiss error"
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
            aria-label="Settings"
            onClick={(e) => e.stopPropagation()}
          >
            <header className="modal-header">
              <h2>Settings</h2>
              <button
                type="button"
                className="modal-close"
                onClick={() => setShowSettings(false)}
                aria-label="Close settings"
              >
                ×
              </button>
            </header>
            <div className="modal-body">
              {!settings ? (
                <p className="modal-loading">Loading…</p>
              ) : (
                <>
                  <section className="settings-section">
                    <h3>Search</h3>
                    <label className="settings-row">
                      <span>Default page size</span>
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
                    <h3>Indexing</h3>
                    <label className="settings-row">
                      <input
                        type="checkbox"
                        checked={settings.index_files}
                        onChange={(e) =>
                          void patchSettings({ index_files: e.target.checked })
                        }
                      />
                      <span>Index files</span>
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
                      <span>Index folders</span>
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
                        Show hidden files
                        <em className="settings-hint">
                          {" "}
                          (basenames starting with <code>.</code>)
                        </em>
                      </span>
                    </label>
                  </section>

                  <section className="settings-section">
                    <h3>Watchers & NAS</h3>
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
                      <span>Restore local-root watchers at startup</span>
                    </label>
                    <label className="settings-row">
                      <span>NAS scan interval (minutes)</span>
                      <input
                        type="number"
                        min={0}
                        placeholder="off"
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
                        Keep index when NAS disconnects
                        <em className="settings-hint">
                          {" "}
                          (always on — see docs/architecture.md)
                        </em>
                      </span>
                    </label>
                  </section>

                  <section className="settings-section">
                    <h3>Storage</h3>
                    <div className="settings-row">
                      <span>Database</span>
                      <code className="settings-path" title={appPaths?.db_path}>
                        {appPaths?.db_path ?? "—"}
                      </code>
                    </div>
                    <div className="settings-row">
                      <span>Logs</span>
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
                        Open
                      </button>
                    </div>
                  </section>

                  <section className="settings-section settings-section-danger">
                    <h3>Danger zone</h3>
                    <p className="settings-danger-note">
                      These actions affect every root and cannot be undone.
                      Each click asks for two confirmations.
                    </p>
                    <div className="settings-row">
                      <button
                        type="button"
                        className="btn-danger"
                        onClick={() => void handleClearIndex()}
                      >
                        Clear index
                      </button>
                      <button
                        type="button"
                        className="btn-danger"
                        onClick={() => void handleRebuildIndex()}
                      >
                        Rebuild all indexes
                      </button>
                    </div>
                  </section>

                  {settingsBusy && (
                    <p className="modal-loading">Saving…</p>
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
