import { useState } from "react";
import type { RootInfo } from "../types";

interface RootSidebarProps {
  roots: RootInfo[];
  loading: boolean;
  selectedRoot: RootInfo | null;
  onSelectRoot: (root: RootInfo | null) => void;
  onScan: (rootId: number) => void;
  onAddRoot: (path: string) => void;
}

function RootSidebar({ roots, loading, selectedRoot, onSelectRoot, onScan, onAddRoot }: RootSidebarProps) {
  const [showAddForm, setShowAddForm] = useState(false);
  const [newPath, setNewPath] = useState("");
  const [adding, setAdding] = useState(false);

  const handleAddRoot = async () => {
    if (!newPath.trim()) return;
    setAdding(true);
    try {
      await onAddRoot(newPath.trim());
      setNewPath("");
      setShowAddForm(false);
    } catch (e) {
      console.error("Failed to add root:", e);
    } finally {
      setAdding(false);
    }
  };

  const getStatusColor = (status: string) => {
    switch (status) {
      case "available":
        return "status-available";
      case "disconnected":
        return "status-disconnected";
      case "scanning":
        return "status-scanning";
      case "error":
        return "status-error";
      default:
        return "";
    }
  };

  const getStatusText = (status: string) => {
    switch (status) {
      case "available":
        return "Available";
      case "disconnected":
        return "Disconnected";
      case "scanning":
        return "Scanning";
      case "error":
        return "Error";
      default:
        return status;
    }
  };

  return (
    <aside className="root-sidebar">
      <div className="sidebar-header">
        <h2>Index Roots</h2>
        <button
          className="add-root-btn"
          onClick={() => setShowAddForm(!showAddForm)}
        >
          + Add Directory
        </button>
      </div>

      {showAddForm && (
        <div className="add-root-form">
          <input
            type="text"
            className="path-input"
            value={newPath}
            onChange={(e) => setNewPath(e.target.value)}
            placeholder="/path/to/directory"
            autoFocus
          />
          <div className="form-actions">
            <button className="submit-btn" onClick={handleAddRoot} disabled={adding}>
              {adding ? "Adding..." : "Add"}
            </button>
            <button className="cancel-btn" onClick={() => setShowAddForm(false)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <div className="root-list">
        {loading ? (
          <div className="loading">Loading roots...</div>
        ) : roots.length === 0 ? (
          <div className="empty-state">No index roots added yet</div>
        ) : (
          roots.map((root) => (
            <div
              key={root.id}
              className={`root-item ${selectedRoot?.id === root.id ? "selected" : ""}`}
              onClick={() => onSelectRoot(root)}
            >
              <div className="root-kind">{root.kind === "nas" ? "NAS" : "Local"}</div>
              <div className="root-info">
                <div className="root-path">{root.path}</div>
                <div className={`root-status ${getStatusColor(root.status)}`}>
                  {getStatusText(root.status)}
                </div>
              </div>
              <button
                className="scan-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onScan(root.id);
                }}
              >
                Scan
              </button>
            </div>
          ))
        )}
      </div>
    </aside>
  );
}

export default RootSidebar;
