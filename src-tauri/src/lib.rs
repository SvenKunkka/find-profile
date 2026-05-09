pub mod db;

use tauri::{command, AppHandle, Manager};
use std::sync::Arc;
use db::{Database, roots, files, scan_jobs, scanner};
use db::roots::{RootKind, RootStatus, Root};
use db::files::{SearchResult, SourceKind};

pub struct AppState {
    db: Arc<Database>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        ApiResponse {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: &str) -> Self {
        ApiResponse {
            success: false,
            data: None,
            error: Some(message.to_string()),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct RootInfo {
    id: i64,
    path: String,
    kind: String,
    enabled: bool,
    status: String,
    scan_interval: i64,
    last_scanned_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

impl From<Root> for RootInfo {
    fn from(root: Root) -> Self {
        RootInfo {
            id: root.id,
            path: root.path,
            kind: root.kind.as_str().to_string(),
            enabled: root.enabled,
            status: root.status.as_str().to_string(),
            scan_interval: root.scan_interval,
            last_scanned_at: root.last_scanned_at,
            created_at: root.created_at,
            updated_at: root.updated_at,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct FileInfo {
    name: String,
    path: String,
    extension: Option<String>,
    size: i64,
    modified_at: i64,
    is_dir: bool,
    source_kind: String,
}

impl From<SearchResult> for FileInfo {
    fn from(result: SearchResult) -> Self {
        FileInfo {
            name: result.name,
            path: result.path,
            extension: result.extension,
            size: result.size,
            modified_at: result.modified_at,
            is_dir: result.is_dir,
            source_kind: result.source_kind.as_str().to_string(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ScanStatus {
    root_id: i64,
    job_id: Option<i64>,
    status: String,
    scanned_count: i64,
    error_count: i64,
    current_path: Option<String>,
}

fn root_kind_to_source_kind(kind: &RootKind) -> SourceKind {
    match kind {
        RootKind::Local => SourceKind::Local,
        RootKind::Nas => SourceKind::Nas,
    }
}

#[command]
fn add_index_root(app: AppHandle, path: String, kind: Option<String>) -> ApiResponse<RootInfo> {
    let state = app.state::<AppState>();
    let root_kind = kind.and_then(|k| RootKind::from_str(&k));
    
    match roots::add_root_with_detection(&state.db, &path, root_kind) {
        Ok(root) => ApiResponse::success(RootInfo::from(root)),
        Err(e) => ApiResponse::error(&format!("Failed to add root: {}", e)),
    }
}

#[command]
fn list_index_roots(app: AppHandle) -> ApiResponse<Vec<RootInfo>> {
    let state = app.state::<AppState>();
    match roots::list_roots(&state.db) {
        Ok(roots) => ApiResponse::success(roots.into_iter().map(RootInfo::from).collect()),
        Err(e) => ApiResponse::error(&format!("Failed to list roots: {}", e)),
    }
}

#[command]
async fn start_scan(app: AppHandle, root_id: i64) -> ApiResponse<ScanStatus> {
    let state = app.state::<AppState>().db.clone();
    let state_clone = state.clone();
    
    let result = roots::get_root_by_id(&state, root_id);
    let root = match result {
        Ok(r) => r,
        Err(e) => return ApiResponse::error(&format!("Root not found: {}", e)),
    };

    let source_kind = root_kind_to_source_kind(&root.kind);
    let path = root.path.clone();

    tokio::spawn(async move {
        let _ = scanner::scan_root(state_clone, root_id, path, source_kind);
    });

    match scan_jobs::get_latest_scan_job(&state, root_id) {
        Ok(Some(job)) => ApiResponse::success(ScanStatus {
            root_id,
            job_id: Some(job.id),
            status: job.status.as_str().to_string(),
            scanned_count: job.scanned_count,
            error_count: job.error_count,
            current_path: None,
        }),
        Ok(None) => ApiResponse::error("No scan job found"),
        Err(e) => ApiResponse::error(&format!("Failed to get scan status: {}", e)),
    }
}

#[command]
fn get_scan_status(app: AppHandle, root_id: i64) -> ApiResponse<ScanStatus> {
    let state = app.state::<AppState>();
    
    match scan_jobs::get_latest_scan_job(&state.db, root_id) {
        Ok(Some(job)) => ApiResponse::success(ScanStatus {
            root_id,
            job_id: Some(job.id),
            status: job.status.as_str().to_string(),
            scanned_count: job.scanned_count,
            error_count: job.error_count,
            current_path: None,
        }),
        Ok(None) => ApiResponse::error("No scan job found"),
        Err(e) => ApiResponse::error(&format!("Failed to get scan status: {}", e)),
    }
}

#[command]
fn search_files(app: AppHandle, query: String, limit: i64, offset: i64) -> ApiResponse<Vec<FileInfo>> {
    let state = app.state::<AppState>();
    match files::search_files(&state.db, &query, Some(limit as usize), Some(offset as usize)) {
        Ok(files) => ApiResponse::success(files.into_iter().map(FileInfo::from).collect()),
        Err(e) => ApiResponse::error(&format!("Failed to search files: {}", e)),
    }
}

#[command]
fn open_file(path: String) -> ApiResponse<()> {
    let status = std::process::Command::new("open")
        .arg(&path)
        .status();
    
    match status {
        Ok(_) => ApiResponse::success(()),
        Err(e) => ApiResponse::error(&format!("Failed to open file: {}", e)),
    }
}

#[command]
fn reveal_in_finder(path: String) -> ApiResponse<()> {
    let status = std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .status();
    
    match status {
        Ok(_) => ApiResponse::success(()),
        Err(e) => ApiResponse::error(&format!("Failed to reveal in finder: {}", e)),
    }
}

#[command]
fn remove_index_root(app: AppHandle, root_id: i64, keep_index: bool) -> ApiResponse<()> {
    let state = app.state::<AppState>();
    
    if !keep_index {
        if let Err(e) = files::remove_files_by_root(&state.db, root_id) {
            return ApiResponse::error(&format!("Failed to remove files: {}", e));
        }
    }
    
    match roots::update_root_status(&state.db, root_id, RootStatus::Error) {
        Ok(_) => ApiResponse::success(()),
        Err(e) => ApiResponse::error(&format!("Failed to remove root: {}", e)),
    }
}

#[command]
async fn rescan_root(app: AppHandle, root_id: i64) -> ApiResponse<ScanStatus> {
    let state = app.state::<AppState>().db.clone();
    let state_clone = state.clone();
    
    let result = roots::get_root_by_id(&state, root_id);
    let root = match result {
        Ok(r) => r,
        Err(e) => return ApiResponse::error(&format!("Root not found: {}", e)),
    };

    let source_kind = root_kind_to_source_kind(&root.kind);
    let path = root.path.clone();

    let _ = files::remove_files_by_root(&state, root_id);

    tokio::spawn(async move {
        let _ = scanner::scan_root(state_clone, root_id, path, source_kind);
    });

    match scan_jobs::get_latest_scan_job(&state, root_id) {
        Ok(Some(job)) => ApiResponse::success(ScanStatus {
            root_id,
            job_id: Some(job.id),
            status: job.status.as_str().to_string(),
            scanned_count: job.scanned_count,
            error_count: job.error_count,
            current_path: None,
        }),
        Ok(None) => ApiResponse::error("No scan job found"),
        Err(e) => ApiResponse::error(&format!("Failed to get scan status: {}", e)),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("find-profile");
    
    std::fs::create_dir_all(&db_dir).ok();
    
    let db_path = db_dir.join("index.db");
    
    let db = Database::new(db_path).expect("Failed to initialize database");
    
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            db: Arc::new(db),
        })
        .invoke_handler(tauri::generate_handler![
            add_index_root,
            list_index_roots,
            start_scan,
            get_scan_status,
            search_files,
            open_file,
            reveal_in_finder,
            remove_index_root,
            rescan_root,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;
    use db::schema;

    fn create_test_db() -> Arc<Database> {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::new(db_path).unwrap();
        schema::init_schema(&db.conn.lock().unwrap()).unwrap();
        Arc::new(db)
    }

    #[test]
    fn test_api_response_success() {
        let response = ApiResponse::success("test");
        assert!(response.success);
        assert_eq!(response.data.unwrap(), "test");
        assert!(response.error.is_none());
    }

    #[test]
    fn test_api_response_error() {
        let response = ApiResponse::<()>::error("error message");
        assert!(!response.success);
        assert!(response.data.is_none());
        assert_eq!(response.error.unwrap(), "error message");
    }

    #[test]
    fn test_root_info_from_root() {
        let root = Root {
            id: 1,
            path: "/test/path".to_string(),
            kind: RootKind::Local,
            enabled: true,
            status: RootStatus::Available,
            scan_interval: 3600,
            last_scanned_at: Some(1234567890),
            created_at: 1234567890,
            updated_at: 1234567890,
        };
        
        let info = RootInfo::from(root);
        assert_eq!(info.id, 1);
        assert_eq!(info.path, "/test/path");
        assert_eq!(info.kind, "local");
        assert_eq!(info.status, "available");
    }

    #[test]
    fn test_root_kind_to_source_kind() {
        assert_eq!(root_kind_to_source_kind(&RootKind::Local), SourceKind::Local);
        assert_eq!(root_kind_to_source_kind(&RootKind::Nas), SourceKind::Nas);
    }

    #[test]
    fn test_db_add_and_list_roots() {
        let db = create_test_db();
        
        let root = roots::add_root(&db, "/test/path", RootKind::Local).unwrap();
        assert_eq!(root.path, "/test/path");
        assert_eq!(root.kind, RootKind::Local);
        
        let root_list = roots::list_roots(&db).unwrap();
        assert_eq!(root_list.len(), 1);
    }

    #[test]
    fn test_db_search_files_empty() {
        let db = create_test_db();
        let results = files::search_files(&db, "test", Some(10), Some(0)).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_db_remove_files_by_root() {
        let db = create_test_db();
        
        let root = roots::add_root(&db, "/test/path", RootKind::Local).unwrap();
        let count = files::remove_files_by_root(&db, root.id).unwrap();
        assert_eq!(count, 0);
    }
}
