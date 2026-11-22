pub mod import;
pub mod lyrics;
pub mod commands;
pub mod cover;
pub mod cue;
pub mod db;
pub mod derive;
pub mod ffmpeg;
pub mod player;
pub mod scan;
pub mod state;
pub mod tags;
pub mod text;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("cover", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            tauri::async_runtime::spawn_blocking(move || {
                let res = match cover::respond(&app, &path) {
                    Some(body) => tauri::http::Response::builder()
                        .status(200)
                        .header("Content-Type", "image/jpeg")
                        .header("Cache-Control", "max-age=31536000, immutable")
                        .body(body),
                    // no cover yet (or its folder is offline): let the webview ask again later
                    None => tauri::http::Response::builder()
                        .status(404)
                        .header("Cache-Control", "no-store")
                        .body(Vec::new()),
                };
                responder.respond(res.unwrap_or_else(|_| tauri::http::Response::new(Vec::new())));
            });
        })
        .setup(|app| {
            let data_dir = app.path().data_dir()?.join("Linger");
            std::fs::create_dir_all(&data_dir)?;
            let db = db::Db::open(&data_dir.join("linger.db"))?;
            // thumbnails of the old `{file_id}.jpg` scheme may belong to another song now
            cover::purge_legacy(&data_dir);
            // all roots start optimistic (online); probing + scanning run in the background
            app.manage(state::AppState::new(db, data_dir));
            commands::player::init(app.handle());
            commands::library::spawn_scan(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::set_app_title,
            commands::library::get_library,
            commands::library::add_roots,
            commands::library::remove_root,
            commands::library::start_scan,
            commands::player::player_play_list,
            commands::player::player_toggle,
            commands::player::player_next,
            commands::player::player_prev,
            commands::player::player_seek,
            commands::player::player_set_volume,
            commands::player::player_set_shuffle,
            commands::player::player_set_repeat,
            commands::player::player_state,
            commands::player::player_queue,
            commands::player::player_queue_add,
            commands::player::player_queue_remove,
            commands::player::player_queue_move,
            commands::player::player_queue_jump,
            commands::player::player_queue_clear,
            commands::playlists::get_playlists,
            commands::playlists::get_playlist,
            commands::playlists::get_favorites,
            commands::playlists::create_playlist,
            commands::playlists::rename_playlist,
            commands::playlists::delete_playlist,
            commands::playlists::playlist_add,
            commands::playlists::playlist_remove,
            commands::playlists::playlist_move,
            commands::playlists::set_favorite,
            commands::playlists::get_saved,
            commands::playlists::get_memberships,
            commands::playlists::save_songs,
            commands::import::import_spotify,
            commands::lyrics::lyrics_get,
            commands::lyrics::lyrics_search,
            commands::lyrics::lyrics_choose,
            commands::lyrics::lyrics_use_file,
            commands::lyrics::lyrics_set_offset,
            commands::lyrics::lyrics_settings,
            commands::lyrics::lyrics_set_settings,
            commands::lyrics::cover_color,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Linger");
}
