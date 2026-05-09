import { useState } from "react";

function App() {
  const [searchQuery, setSearchQuery] = useState("");

  return (
    <div className="app-container">
      <header className="search-header">
        <div className="header-left">
          <h1 className="app-title">Find Profile</h1>
        </div>
        <div className="search-box">
          <svg className="search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <circle cx="11" cy="11" r="8" />
            <path d="m21 21-4.35-4.35" />
          </svg>
          <input
            type="text"
            className="search-input"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search files..."
            autoFocus
          />
          <span className="result-count">0 results</span>
        </div>
      </header>

      <div className="main-content">
        <aside className="root-sidebar">
          <div className="sidebar-header">
            <h2>Indexed Roots</h2>
            <button className="btn-add">+</button>
          </div>
          <div className="root-list">
            <div className="empty-roots">
              <p>No indexed roots</p>
              <p className="hint">Click + to add a directory</p>
            </div>
          </div>
        </aside>

        <main className="file-table-container">
          <div className="empty-state">
            <svg className="empty-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="17 8 12 3 7 8" />
              <line x1="12" y1="3" x2="12" y2="15" />
            </svg>
            <p>Results</p>
            <p className="hint">Search to find files</p>
          </div>
        </main>
      </div>

      <footer className="status-bar">
        <div className="status-left">
          <span className="current-root">Root: None</span>
        </div>
        <div className="status-center">
          <span className="idle">Status: Ready</span>
        </div>
        <div className="status-right">
          <span className="current-path">Path: None</span>
        </div>
      </footer>
    </div>
  );
}

export default App;
