pub mod commands;
pub mod db;
pub mod roots;
pub mod scanner;
pub mod watcher;

use tauri::Manager;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("find_profile.db");
            let conn = db::init_db(&db_path)?;
            let state = AppState::new(conn);

            // Restore filesystem watchers for previously-added enabled local
            // roots. Failures are logged via stderr — they must not block
            // startup. NAS roots are skipped by `should_watch_root`.
            match state.watchers.restore_all(state.conn.clone()) {
                Ok(n) => eprintln!("watcher: restored {n} local root watcher(s) at startup"),
                Err(e) => eprintln!("watcher: restore_all failed at startup: {e}"),
            }

            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::add_index_root,
            commands::list_index_roots,
            commands::start_scan,
            commands::get_scan_status,
            commands::search_files,
            commands::open_file,
            commands::reveal_in_finder,
            commands::remove_index_root,
            commands::rescan_root,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
