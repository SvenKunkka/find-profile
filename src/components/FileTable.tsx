import React from "react";
import type { FileInfo } from "../types";

interface FileTableProps {
  files: FileInfo[];
  loading: boolean;
  onDoubleClick: (file: FileInfo) => void;
  onContextMenu: (e: React.MouseEvent, file: FileInfo) => void;
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`;
}

function formatDate(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  return date.toLocaleString();
}

function FileTable({ files, loading, onDoubleClick, onContextMenu }: FileTableProps) {
  if (loading) {
    return (
      <main className="file-table-container">
        <div className="loading-overlay">Searching...</div>
      </main>
    );
  }

  if (files.length === 0) {
    return (
      <main className="file-table-container">
        <div className="empty-state">
          <svg className="empty-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
            <polyline points="17 8 12 3 7 8" />
            <line x1="12" y1="3" x2="12" y2="15" />
          </svg>
          <p>No files found</p>
          <p className="hint">Try a different search query or add an index root</p>
        </div>
      </main>
    );
  }

  return (
    <main className="file-table-container">
      <table className="file-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Path</th>
            <th>Extension</th>
            <th>Size</th>
            <th>Modified</th>
            <th>Type</th>
            <th>Source</th>
          </tr>
        </thead>
        <tbody>
          {files.map((file, index) => (
            <tr
              key={index}
              className={`file-row ${file.is_dir ? "is-directory" : ""}`}
              onDoubleClick={() => onDoubleClick(file)}
              onContextMenu={(e) => onContextMenu(e, file)}
            >
              <td className="name-cell">
                <svg className="file-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                  {file.is_dir ? (
                    <>
                      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                    </>
                  ) : (
                    <>
                      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                      <polyline points="14 2 14 8 20 8" />
                      <line x1="16" y1="13" x2="8" y2="13" />
                      <line x1="16" y1="17" x2="8" y2="17" />
                      <polyline points="10 9 9 9 8 9" />
                    </>
                  )}
                </svg>
                <span>{file.name}</span>
              </td>
              <td className="path-cell">{file.path}</td>
              <td className="extension-cell">{file.extension || "-"}</td>
              <td className="size-cell">{file.is_dir ? "-" : formatSize(file.size)}</td>
              <td className="date-cell">{formatDate(file.modified_at)}</td>
              <td className="type-cell">{file.is_dir ? "Directory" : "File"}</td>
              <td className="source-cell">{file.source_kind}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </main>
  );
}

export default FileTable;
