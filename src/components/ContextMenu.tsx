import { useEffect } from "react";
import type { FileInfo } from "../types";

interface ContextMenuProps {
  x: number;
  y: number;
  file: FileInfo;
  onAction: (action: string) => void;
  onClose: () => void;
}

function ContextMenu({ x, y, file, onAction, onClose }: ContextMenuProps) {
  useEffect(() => {
    const handleClickOutside = (_e: MouseEvent) => {
      onClose();
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };

    document.addEventListener("click", handleClickOutside);
    document.addEventListener("keydown", handleKeyDown);

    return () => {
      document.removeEventListener("click", handleClickOutside);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [onClose]);

  return (
    <div
      className="context-menu"
      style={{ left: `${x}px`, top: `${y}px` }}
      onClick={(e) => e.stopPropagation()}
    >
      <div className="menu-header">{file.name}</div>
      <div className="menu-separator"></div>
      <button className="menu-item" onClick={() => onAction("open")}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" className="menu-icon">
          <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8m6-2.74a9.75 9.75 0 0 0 6.74 2.74M3 3h3m3 0h3m-9 3h.01M9 12a3 3 0 1 0 0-6 3 3 0 0 0 0 6zm12-3a3 3 0 1 1-6 0 3 3 0 0 1 6 0zm-9 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zm12-3a3 3 0 1 1-6 0 3 3 0 0 1 6 0z" />
        </svg>
        Open
      </button>
      <button className="menu-item" onClick={() => onAction("reveal")}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" className="menu-icon">
          <path d="M15 21h5v-2a3 3 0 0 0-5.356-1.857M15 21H9m12 0a6 6 0 0 1-12 0" />
          <path d="M21 3h-1a6 6 0 0 0-5.127 2.879M21 3H3m18 0V1m0 2v2" />
        </svg>
        Reveal in Finder
      </button>
      <button className="menu-item" onClick={() => onAction("copy")}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" className="menu-icon">
          <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
          <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
        </svg>
        Copy Path
      </button>
    </div>
  );
}

export default ContextMenu;
