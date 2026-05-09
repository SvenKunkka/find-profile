import type { ScanStatus, RootInfo } from "../types";

interface StatusBarProps {
  status: ScanStatus | null;
  root: RootInfo | null;
}

function StatusBar({ status, root }: StatusBarProps) {
  return (
    <footer className="status-bar">
      <div className="status-left">
        {root && (
          <span className="current-root">
            Root: <span className="root-path">{root.path}</span>
          </span>
        )}
      </div>
      <div className="status-center">
        {status ? (
          <span className={`scan-status ${status.status}`}>
            {status.status === "scanning" && (
              <span className="spinner"></span>
            )}
            {status.status.charAt(0).toUpperCase() + status.status.slice(1)}
            {status.scanned_count > 0 && (
              <span className="scan-stats">
                ({status.scanned_count} scanned, {status.error_count} errors)
              </span>
            )}
          </span>
        ) : (
          <span className="idle">Ready</span>
        )}
      </div>
      <div className="status-right">
        {status?.current_path && (
          <span className="current-path">
            Scanning: {status.current_path}
          </span>
        )}
      </div>
    </footer>
  );
}

export default StatusBar;
