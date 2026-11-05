pub mod commands;
pub mod cue;
pub mod db;
pub mod derive;
pub mod ffmpeg;
pub mod scan;
pub mod state;
pub mod tags;
pub mod text;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().data_dir()?.join("Linger");
            std::fs::create_dir_all(&data_dir)?;
            let db = db::Db::open(&data_dir.join("linger.db"))?;
            // all roots start optimistic (online); probing + scanning run in the background
            app.manage(state::AppState::new(db, data_dir));
            commands::library::spawn_scan(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::library::get_library,
            commands::library::add_roots,
            commands::library::remove_root,
            commands::library::start_scan,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Linger");
}
