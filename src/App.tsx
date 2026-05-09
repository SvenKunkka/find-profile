import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type RootKind = "local" | "nas";
type RootStatus = "available" | "disconnected" | "scanning" | "error";

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

interface CommandError {
  code: string;
  message: string;
}

const SEARCH_DEBOUNCE_MS = 200;
const SEARCH_LIMIT = 200;
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
  // A NAS root that's disconnected (or any root in error state) makes its
  // cached files unreachable right now even if `is_accessible` was true at
  // scan time. Surface that as `unavailable` so users don't think a stale
  // entry is openable.
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

function App() {
  const [roots, setRoots] = useState<Root[]>([]);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<FileHit[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [currentRoot, setCurrentRoot] = useState<Root | null>(null);
  const [currentPath, setCurrentPath] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);

  const searchTimer = useRef<number | null>(null);

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

  // Initial load: never blocks initial render.
  useEffect(() => {
    void refreshRoots();
  }, []);

  // Debounced search.
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
            limit: SEARCH_LIMIT,
            offset: 0,
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
  }, [query]);

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
      return; // user cancelled
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

  async function handleScan(rootId: number) {
    setError(null);
    try {
      await invoke("start_scan", { rootId });
      await refreshRoots();

      // Light polling: scanner flips status to Scanning at start and back to
      // Available on finish. We refresh until the scan_job for this root is
      // no longer running, with a hard ceiling so we never poll forever.
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

  async function handleReveal(path: string) {
    setError(null);
    try {
      await invoke("reveal_in_finder", { path });
    } catch (e) {
      console.error("reveal_in_finder failed", e);
      setError(`Failed to reveal in Finder: ${formatError(e)}`);
    }
  }

  const resultCountLabel = (() => {
    if (query.trim().length === 0) return "";
    if (!searched) return "…";
    return `${results.length} result${results.length === 1 ? "" : "s"}`;
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
                      className="btn-scan"
                      onClick={(e) => {
                        e.stopPropagation();
                        void handleScan(r.id);
                      }}
                      disabled={r.status === "scanning"}
                    >
                      {r.status === "scanning" ? "Scanning…" : "Scan"}
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
          {results.length === 0 ? (
            <div className="panel-empty">No results.</div>
          ) : (
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
          )}
        </section>
      </main>

      <footer className="status-bar" aria-label="Status">
        <div className="status-cell">
          <span className="status-label">Status</span>
          <span className="status-value">
            Ready · {roots.length} root{roots.length === 1 ? "" : "s"}
            {results.length > 0
              ? ` · ${results.length} hit${results.length === 1 ? "" : "s"}`
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
    </div>
  );
}

export default App;
