pub mod commands;
pub mod db;
pub mod roots;
pub mod scanner;
pub mod settings;
pub mod watcher;

use tauri::Manager;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // ---- file logging --------------------------------------------------
            let log_dir = app.path().app_log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            let appender =
                tracing_appender::rolling::daily(&log_dir, "find_profile.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            // The non-blocking writer's worker thread is kept alive by `guard`;
            // intentionally leak it so it lives for the full app lifetime
            // without us needing a global slot for it.
            std::mem::forget(guard);
            let _ = tracing_subscriber::fmt()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(true)
                .with_level(true)
                .with_max_level(tracing::Level::INFO)
                .try_init();
            tracing::info!(
                log_dir = %log_dir.display(),
                "find profile starting",
            );

            // ---- database ------------------------------------------------------
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("find_profile.db");
            tracing::info!(db_path = %db_path.display(), "initializing database");
            let conn = db::init_db(&db_path)?;

            // ---- settings ------------------------------------------------------
            let app_settings = settings::load(&conn).unwrap_or_default();
            tracing::info!(?app_settings, "loaded settings");

            let state = AppState::new(conn);

            // ---- restore watchers (gated by setting) --------------------------
            if app_settings.restore_watchers_on_startup {
                match state.watchers.restore_all(state.conn.clone()) {
                    Ok(n) => tracing::info!(count = n, "restored local watchers"),
                    Err(e) => tracing::warn!(error = %e, "watcher restore_all failed"),
                }
            } else {
                tracing::info!("watcher restoration disabled by settings; skipping");
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
            commands::cancel_scan,
            commands::start_scan_all_roots,
            commands::get_settings,
            commands::update_settings,
            commands::get_app_paths,
            commands::clear_index,
            commands::rebuild_index,
            commands::open_log_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
