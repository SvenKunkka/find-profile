pub mod commands;
pub mod db;
pub mod roots;
pub mod scanner;

use tauri::Manager;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("find_profile.db");
            let conn = db::init_db(&db_path)?;
            app.manage(AppState::new(conn));
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
