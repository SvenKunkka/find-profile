import { invoke } from "@tauri-apps/api/core";
import { useState, useCallback, useEffect } from "react";
import type { ApiResponse, RootInfo, FileInfo, ScanStatus } from "../types";

export function useApi<T>(command: string, params: Record<string, unknown> = {}) {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<ApiResponse<T>>(command, params);
      if (result.success && result.data) {
        setData(result.data);
      } else {
        setError(result.error || "Unknown error");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }, [command, params]);

  return { data, loading, error, fetchData };
}

export function useIndexRoots() {
  const [roots, setRoots] = useState<RootInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadRoots = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<ApiResponse<RootInfo[]>>("list_index_roots");
      if (result.success && result.data) {
        setRoots(result.data);
      } else {
        setError(result.error || "Failed to load roots");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadRoots();
  }, [loadRoots]);

  const addRoot = useCallback(async (path: string, kind?: string) => {
    const result = await invoke<ApiResponse<RootInfo>>("add_index_root", {
      path,
      kind: kind || null,
    });
    if (result.success && result.data) {
      await loadRoots();
      return result.data;
    }
    throw new Error(result.error || "Failed to add root");
  }, [loadRoots]);

  const removeRoot = useCallback(async (rootId: number, keepIndex: boolean) => {
    const result = await invoke<ApiResponse<void>>("remove_index_root", {
      root_id: rootId,
      keep_index: keepIndex,
    });
    if (result.success) {
      await loadRoots();
    } else {
      throw new Error(result.error || "Failed to remove root");
    }
  }, [loadRoots]);

  return { roots, loading, error, loadRoots, addRoot, removeRoot };
}

export function useSearch(query: string, limit = 50, offset = 0) {
  const [files, setFiles] = useState<FileInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const search = useCallback(async () => {
    if (!query.trim()) {
      setFiles([]);
      return;
    }

    setLoading(true);
    setError(null);
    try {
      const result = await invoke<ApiResponse<FileInfo[]>>("search_files", {
        query,
        limit,
        offset,
      });
      if (result.success && result.data) {
        setFiles(result.data);
      } else {
        setError(result.error || "Search failed");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }, [query, limit, offset]);

  return { files, loading, error, search };
}

export function useScanStatus(rootId: number) {
  const [status, setStatus] = useState<ScanStatus | null>(null);
  const [loading, setLoading] = useState(false);

  const getStatus = useCallback(async () => {
    setLoading(true);
    try {
      const result = await invoke<ApiResponse<ScanStatus>>("get_scan_status", {
        root_id: rootId,
      });
      if (result.success && result.data) {
        setStatus(result.data);
      }
    } catch (e) {
      console.error("Failed to get scan status:", e);
    } finally {
      setLoading(false);
    }
  }, [rootId]);

  const startScan = useCallback(async () => {
    const result = await invoke<ApiResponse<ScanStatus>>("start_scan", {
      root_id: rootId,
    });
    if (result.success && result.data) {
      setStatus(result.data);
    } else {
      throw new Error(result.error || "Failed to start scan");
    }
  }, [rootId]);

  const rescanRoot = useCallback(async () => {
    const result = await invoke<ApiResponse<ScanStatus>>("rescan_root", {
      root_id: rootId,
    });
    if (result.success && result.data) {
      setStatus(result.data);
    } else {
      throw new Error(result.error || "Failed to rescan");
    }
  }, [rootId]);

  return { status, loading, getStatus, startScan, rescanRoot };
}

export async function openFile(path: string): Promise<void> {
  const result = await invoke<ApiResponse<void>>("open_file", { path });
  if (!result.success) {
    throw new Error(result.error || "Failed to open file");
  }
}

export async function revealInFinder(path: string): Promise<void> {
  const result = await invoke<ApiResponse<void>>("reveal_in_finder", { path });
  if (!result.success) {
    throw new Error(result.error || "Failed to reveal in finder");
  }
}
