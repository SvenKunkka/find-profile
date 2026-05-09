function App() {
  function handleAddRoot() {
    console.log("add root clicked");
    // TODO(next stage): open Tauri directory picker and call add_index_root.
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
          <div className="panel-empty">No roots yet.</div>
        </aside>

        <section className="results-panel" aria-label="Results">
          <div className="panel-header">
            <h2 className="panel-title">Results</h2>
          </div>
          <div className="panel-empty">No results.</div>
        </section>
      </main>

      <footer className="status-bar">
        <span className="status-text">Status: Ready</span>
      </footer>
    </div>
  );
}

export default App;
