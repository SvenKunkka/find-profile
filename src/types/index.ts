export interface ApiResponse<T> {
  success: boolean;
  data: T | null;
  error: string | null;
}

export interface RootInfo {
  id: number;
  path: string;
  kind: string;
  enabled: boolean;
  status: string;
  scan_interval: number;
  last_scanned_at: number | null;
  created_at: number;
  updated_at: number;
}

export interface FileInfo {
  name: string;
  path: string;
  extension: string | null;
  size: number;
  modified_at: number;
  is_dir: boolean;
  source_kind: string;
}

export interface ScanStatus {
  root_id: number;
  job_id: number | null;
  status: string;
  scanned_count: number;
  error_count: number;
  current_path: string | null;
}

export interface SearchResult {
  files: FileInfo[];
  total: number;
}
