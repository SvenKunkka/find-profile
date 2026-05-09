import { useEffect, useState } from "react";
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

interface CommandError {
  code: string;
  message: string;
}

function formatError(err: unknown): string {
  if (err == null) return "unknown error";
  if (typeof err === "string") return err;
  if (typeof err === "object") {
    const maybe = err as Partial<CommandError> & { toString?: () => string };
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

function App() {
  const [roots, setRoots] = useState<Root[]>([]);
  const [error, setError] = useState<string | null>(null);

  async function refreshRoots() {
    try {
      const next = await invoke<Root[]>("list_index_roots");
      setRoots(next);
    } catch (e) {
      console.error("list_index_roots failed", e);
      setError(`Failed to load roots: ${formatError(e)}`);
    }
  }

  useEffect(() => {
    void refreshRoots();
  }, []);

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

    // User cancelled the picker — silently bail, no error.
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

  return (
    <div className="app">
      <header className="app-header">
        <h1 className="app-title">Find Profile</h1>
      </header>

      <section className="search-bar">
        <input
          type="text"
          className="search-input"
          placeholder="Search files..."
          aria-label="Search files"
        />
      </section>

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
                <li key={r.id} className="root-item">
                  <div className="root-path" title={r.path}>
                    {r.path}
                  </div>
                  <div className="root-meta">
                    <span className={`root-kind kind-${r.kind}`}>{r.kind}</span>
                    <span className={`root-status status-${r.status}`}>
                      {r.status}
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </aside>

        <section className="results-panel" aria-label="Results">
          <div className="panel-header">
            <h2 className="panel-title">Results</h2>
          </div>
          <div className="panel-empty">No results.</div>
        </section>
      </main>

      <footer className="status-bar">
        {error ? (
          <span className="status-text status-error" role="alert">
            {error}
            <button
              type="button"
              className="status-dismiss"
              onClick={() => setError(null)}
              aria-label="Dismiss error"
            >
              ×
            </button>
          </span>
        ) : (
          <span className="status-text">
            Status: Ready
            {roots.length > 0 ? ` · ${roots.length} root${roots.length === 1 ? "" : "s"}` : ""}
          </span>
        )}
      </footer>
    </div>
  );
}

export default App;
