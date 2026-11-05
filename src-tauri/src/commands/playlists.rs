//! Playlist and favorite commands. Logic lives in `do_*` functions over `&mut Db` so it is
//! testable without a Tauri runtime; the commands lock, read the clock, call, and emit.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::db::{Db, DbError, Snapshot};
use crate::state::AppState;

const NOT_FOUND: &str = "playlist.notFound";
const MAX_NAME: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaylistDto { pub id: i64, pub name: String, pub count: usize, pub dur: f64 }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemDto { pub i: i64, pub id: String, pub t: String, pub a: String, pub b: String, pub s: f64, pub ad: i64 }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddDto { pub added: usize, pub duplicate: usize }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MembershipDto { pub total: usize, pub fav: usize, pub lists: Vec<(i64, usize)> }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SaveDto { pub added: usize, pub duplicate: usize, pub removed: usize }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PlaylistsChanged { pub id: Option<i64>, pub favorites: bool }

fn err(e: DbError) -> String {
    match e {
        DbError::NotFound => NOT_FOUND.to_string(),
        e => e.to_string(),
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn validate_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("playlist.nameEmpty".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err("playlist.nameTooLong".into());
    }
    Ok(n.to_string())
}

fn item(i: i64, id: String, s: Snapshot, ad: i64) -> ItemDto {
    ItemDto { i, id, t: s.title, a: s.artist, b: s.album, s: s.duration, ad }
}

fn require(db: &Db, id: i64) -> Result<(), String> {
    if db.playlist_exists(id).map_err(err)? { Ok(()) } else { Err(NOT_FOUND.into()) }
}

fn dto_of(db: &Db, id: i64) -> Result<PlaylistDto, String> {
    db.playlists().map_err(err)?.into_iter().find(|p| p.id == id)
        .map(|p| PlaylistDto { id: p.id, name: p.name, count: p.count, dur: p.duration })
        .ok_or_else(|| NOT_FOUND.to_string())
}

pub fn do_playlists(db: &Db) -> Result<Vec<PlaylistDto>, String> {
    Ok(db.playlists().map_err(err)?.into_iter()
        .map(|p| PlaylistDto { id: p.id, name: p.name, count: p.count, dur: p.duration }).collect())
}

pub fn do_items(db: &Db, id: i64) -> Result<Vec<ItemDto>, String> {
    require(db, id)?;
    Ok(db.playlist_items(id).map_err(err)?.into_iter().map(|i| item(i.item_id, i.song_id, i.snap, i.added)).collect())
}

pub fn do_favorites(db: &Db) -> Result<Vec<ItemDto>, String> {
    Ok(db.favorites().map_err(err)?.into_iter().map(|f| item(0, f.song_id, f.snap, f.added)).collect())
}

pub fn do_create(db: &mut Db, name: &str, ids: &[String], now: i64) -> Result<PlaylistDto, String> {
    let name = validate_name(name)?;
    let id = db.create_playlist(&name, now).map_err(err)?;
    if !ids.is_empty() {
        db.playlist_add(id, ids, now).map_err(err)?;
    }
    dto_of(db, id)
}

pub fn do_rename(db: &mut Db, id: i64, name: &str, now: i64) -> Result<(), String> {
    let name = validate_name(name)?;
    if db.rename_playlist(id, &name, now).map_err(err)? { Ok(()) } else { Err(NOT_FOUND.into()) }
}

pub fn do_delete(db: &mut Db, id: i64) -> Result<(), String> {
    if db.delete_playlist(id).map_err(err)? { Ok(()) } else { Err(NOT_FOUND.into()) }
}

pub fn do_add(db: &mut Db, id: i64, ids: &[String], now: i64) -> Result<AddDto, String> {
    require(db, id)?;
    let o = db.playlist_add(id, ids, now).map_err(err)?;
    Ok(AddDto { added: o.added, duplicate: o.duplicate })
}

pub fn do_remove(db: &mut Db, id: i64, item_ids: &[i64]) -> Result<(), String> {
    require(db, id)?;
    db.playlist_remove(id, item_ids).map_err(err)?;
    Ok(())
}

pub fn do_move(db: &mut Db, id: i64, item_id: i64, to: usize) -> Result<(), String> {
    require(db, id)?;
    db.playlist_move(id, item_id, to).map_err(err)?;
    Ok(())
}

pub fn do_memberships(db: &Db, ids: &[String]) -> Result<MembershipDto, String> {
    let (fav, lists) = db.memberships(ids).map_err(err)?;
    Ok(MembershipDto { total: ids.len(), fav, lists })
}

pub fn do_save(db: &mut Db, ids: &[String], add: &[i64], remove: &[i64], now: i64) -> Result<SaveDto, String> {
    let (added, duplicate, removed) = db.save_songs(ids, add, remove, now).map_err(err)?;
    Ok(SaveDto { added, duplicate, removed })
}

fn emit(app: &AppHandle, id: Option<i64>, favorites: bool) {
    let _ = app.emit("playlists://changed", PlaylistsChanged { id, favorites });
}

#[tauri::command]
pub fn get_playlists(state: State<'_, AppState>) -> Result<Vec<PlaylistDto>, String> {
    do_playlists(&state.db())
}

#[tauri::command]
pub fn get_playlist(id: i64, state: State<'_, AppState>) -> Result<Vec<ItemDto>, String> {
    do_items(&state.db(), id)
}

#[tauri::command]
pub fn get_favorites(state: State<'_, AppState>) -> Result<Vec<ItemDto>, String> {
    do_favorites(&state.db())
}

#[tauri::command]
pub fn create_playlist(name: String, song_ids: Vec<String>, app: AppHandle, state: State<'_, AppState>) -> Result<PlaylistDto, String> {
    let dto = do_create(&mut state.db(), &name, &song_ids, now())?;
    emit(&app, Some(dto.id), false);
    Ok(dto)
}

#[tauri::command]
pub fn rename_playlist(id: i64, name: String, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    do_rename(&mut state.db(), id, &name, now())?;
    emit(&app, Some(id), false);
    Ok(())
}

#[tauri::command]
pub fn delete_playlist(id: i64, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    do_delete(&mut state.db(), id)?;
    emit(&app, Some(id), false);
    Ok(())
}

#[tauri::command]
pub fn playlist_add(id: i64, song_ids: Vec<String>, app: AppHandle, state: State<'_, AppState>) -> Result<AddDto, String> {
    let dto = do_add(&mut state.db(), id, &song_ids, now())?;
    emit(&app, Some(id), false);
    Ok(dto)
}

#[tauri::command]
pub fn playlist_remove(id: i64, item_ids: Vec<i64>, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    do_remove(&mut state.db(), id, &item_ids)?;
    emit(&app, Some(id), false);
    Ok(())
}

#[tauri::command]
pub fn playlist_move(id: i64, item_id: i64, to: usize, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    do_move(&mut state.db(), id, item_id, to)?;
    emit(&app, Some(id), false);
    Ok(())
}

#[tauri::command]
pub fn set_favorite(song_ids: Vec<String>, on: bool, app: AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let n = state.db().set_favorites(&song_ids, on, now()).map_err(err)?;
    emit(&app, None, true);
    Ok(n)
}

#[tauri::command]
pub fn get_saved(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state.db().saved_song_ids().map_err(err)
}

#[tauri::command]
pub fn get_memberships(song_ids: Vec<String>, state: State<'_, AppState>) -> Result<MembershipDto, String> {
    do_memberships(&state.db(), &song_ids)
}

#[tauri::command]
pub fn save_songs(song_ids: Vec<String>, add: Vec<i64>, remove: Vec<i64>, app: AppHandle, state: State<'_, AppState>) -> Result<SaveDto, String> {
    let dto = do_save(&mut state.db(), &song_ids, &add, &remove, now())?;
    emit(&app, None, true);
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{song_id, FileMeta, NewSong};

    fn lib(db: &mut Db) -> Vec<String> {
        let r = db.add_root("D:/m").unwrap();
        let m = FileMeta { rel: "x.flac".into(), ext: "flac".into(), size: 7, mtime: 1, codec: "flac".into(),
            bitrate: 900, duration: 10.0, cover: 0, cue_sig: None };
        let s = |t: &str, a: &str, st: u64, d: f64| NewSong { id: song_id("D:/m", "x.flac", st), title: t.into(),
            artist: a.into(), album: "B".into(), track: 1, start_ms: st, end_ms: None, duration: d };
        db.replace_file(r.id, &m, &[s("晴天", "周杰伦", 0, 2.5), s("Yesterday", "The Beatles", 3000, 4.0)]).unwrap();
        vec![song_id("D:/m", "x.flac", 0), song_id("D:/m", "x.flac", 3000)]
    }

    #[test]
    fn validate_name_rules() {
        assert_eq!(validate_name("  晴天 "), Ok("晴天".to_string()));
        assert_eq!(validate_name("   "), Err("playlist.nameEmpty".to_string()));
        assert_eq!(validate_name(&"字".repeat(101)), Err("playlist.nameTooLong".to_string()));
        assert!(validate_name(&"字".repeat(100)).is_ok());
    }

    #[test]
    fn create_with_songs_returns_count() {
        let mut db = Db::open_in_memory().unwrap();
        let ids = lib(&mut db);
        let dto = do_create(&mut db, "x", &[ids[0].clone(), ids[1].clone(), ids[0].clone()], 1).unwrap();
        assert_eq!((dto.count, dto.dur, dto.name.as_str()), (2, 6.5, "x"));
    }

    #[test]
    fn items_dto_uses_snapshot() {
        let mut db = Db::open_in_memory().unwrap();
        let ids = lib(&mut db);
        let p = do_create(&mut db, "x", &ids, 5).unwrap();
        db.set_favorites(&ids[..1], true, 9).unwrap();
        db.remove_root(db.roots().unwrap()[0].id).unwrap();
        let items = do_items(&db, p.id).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].t.as_str(), items[0].a.as_str(), items[0].b.as_str(), items[0].s, items[0].ad),
                   ("晴天", "周杰伦", "B", 2.5, 5));
        assert!(items[0].i > 0);
        let fav = do_favorites(&db).unwrap();
        assert_eq!((fav[0].i, fav[0].t.as_str()), (0, "晴天"));
    }

    #[test]
    fn memberships_dto_counts() {
        let mut db = Db::open_in_memory().unwrap();
        let ids = lib(&mut db);
        let p = do_create(&mut db, "x", &ids[..1], 1).unwrap();
        db.set_favorites(&ids, true, 1).unwrap();
        let m = do_memberships(&db, &ids).unwrap();
        let (fav, lists) = db.memberships(&ids).unwrap();
        assert_eq!((m.total, m.fav, m.lists.clone()), (2, fav, lists));
        assert_eq!(m.lists, vec![(p.id, 1)]);
    }

    #[test]
    fn missing_playlist_is_not_found() {
        let mut db = Db::open_in_memory().unwrap();
        let ids = lib(&mut db);
        let nf = "playlist.notFound".to_string();
        assert_eq!(do_items(&db, 999).unwrap_err(), nf);
        assert_eq!(do_add(&mut db, 999, &ids, 1).unwrap_err(), nf);
        assert_eq!(do_rename(&mut db, 999, "n", 1).unwrap_err(), nf);
        assert_eq!(do_delete(&mut db, 999).unwrap_err(), nf);
        assert_eq!(do_save(&mut db, &ids, &[999], &[], 1).unwrap_err(), nf);
    }
}
