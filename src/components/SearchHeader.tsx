interface SearchHeaderProps {
  query: string;
  onQueryChange: (query: string) => void;
  resultCount: number;
}

function SearchHeader({ query, onQueryChange, resultCount }: SearchHeaderProps) {
  return (
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
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          placeholder="Search files..."
          autoFocus
        />
        <span className="result-count">{resultCount} results</span>
      </div>
    </header>
  );
}

export default SearchHeader;
