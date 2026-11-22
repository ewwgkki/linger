//! Lyrics commands. Network and file IO never run under the db lock, and run on the blocking
//! pool so a slow lookup cannot starve the async workers. Logic lives in plain functions for testing.

use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::db::{Db, LyricRow, SongRow};
use crate::lyrics::online::{Candidate, Provider, Query};
use crate::lyrics::resolve::{parsed_of, resolve, Resolved};
use crate::lyrics::{lrclib::Lrclib, local::decode_lrc, netease::Netease, parse_lrc, Line};
use crate::state::AppState;

const NOT_FOUND: &str = "lyrics.notFound";
const NET_ERROR: &str = "lyrics.netError";
const BAD_FILE: &str = "lyrics.badFile";
const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_OFFSET: i32 = 30_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LyricsDto {
    pub id: String,
    pub status: String,
    pub kind: String,
    pub source: Option<String>,
    pub lines: Vec<Line>,
    pub offset: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LyricsSettings { pub netease: bool, pub trans: bool }

pub fn clamp_offset(ms: i32) -> i32 { ms.clamp(-MAX_OFFSET, MAX_OFFSET) }

pub fn provider_names(netease: bool) -> Vec<&'static str> {
    if netease { vec!["lrclib", "netease"] } else { vec!["lrclib"] }
}

fn providers_for(names: &[&str]) -> Vec<Box<dyn Provider>> {
    names.iter().filter_map(|n| match *n {
        "lrclib" => Some(Box::new(Lrclib) as Box<dyn Provider>),
        "netease" => Some(Box::new(Netease) as Box<dyn Provider>),
        _ => None,
    }).collect()
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn dto_of(id: &str, r: Resolved, offset: i32) -> LyricsDto {
    LyricsDto {
        id: id.to_string(), status: r.status.to_string(), kind: r.parsed.kind().to_string(),
        source: r.source, lines: r.parsed.lines, offset,
    }
}

fn read_settings(db: &Db) -> LyricsSettings {
    let get = |k: &str| db.setting(k).ok().flatten().map(|v| v != "false").unwrap_or(true);
    LyricsSettings { netease: get("lyrics_netease"), trans: get("lyrics_trans") }
}

/// (song, audio path when its folder is online)
fn locate(state: &AppState, id: &str) -> Result<(SongRow, Option<std::path::PathBuf>), String> {
    let (song, path) = {
        let db = state.db();
        let song = db.song(id).ok().flatten().ok_or(NOT_FOUND)?;
        let roots = db.roots().map_err(|e| e.to_string())?;
        let path = super::player::song_path(&roots, &song);
        (song, path)
    };
    let audio = if state.is_online(song.root_id) { path } else { None };
    Ok((song, audio))
}

fn require_song(db: &Db, id: &str) -> Result<SongRow, String> {
    db.song(id).ok().flatten().ok_or_else(|| NOT_FOUND.to_string())
}

fn query_of(song: &SongRow, title: Option<String>, artist: Option<String>) -> Query {
    // None: use the tag. A field the user cleared stays blank.
    let pick = |o: Option<String>, d: &str| o.map_or_else(|| d.to_string(), |s| s.trim().to_string());
    Query { title: pick(title, &song.title), artist: pick(artist, &song.artist), album: song.album.clone(), duration: song.duration }
}

pub fn search_with(providers: &[&dyn Provider], q: &Query) -> Result<Vec<Candidate>, String> {
    if q.title.trim().is_empty() && q.artist.trim().is_empty() { return Ok(Vec::new()); }
    let mut out = Vec::new();
    let mut failed = 0;
    for p in providers {
        match p.search(q) {
            Ok(mut v) => out.append(&mut v),
            Err(_) => failed += 1,
        }
    }
    if !providers.is_empty() && failed == providers.len() { return Err(NET_ERROR.into()); }
    Ok(out)
}

/// Fetches `ext_id` from the named provider and returns the row to pin.
pub fn fetch_pinned(providers: &[&dyn Provider], source: &str, ext_id: &str, now: i64) -> Result<LyricRow, String> {
    let p = providers.iter().find(|p| p.name() == source).ok_or(NET_ERROR)?;
    let raw = p.fetch(ext_id).map_err(|_| NET_ERROR)?.ok_or(NOT_FOUND)?;
    let row = LyricRow {
        status: "ok".into(), source: Some(p.name().to_string()), ext_id: Some(ext_id.to_string()),
        lrc: raw.lrc, yrc: raw.yrc, trans: raw.trans, plain: raw.plain, pinned: true, fetched: now,
    };
    if parsed_of(&row).is_empty() { return Err(NOT_FOUND.into()); }
    Ok(row)
}

/// Reads (never writes) a user-chosen lyric file and returns the row to pin.
pub fn row_from_file(path: &Path, now: i64) -> Result<LyricRow, String> {
    let f = std::fs::File::open(path).map_err(|_| BAD_FILE)?;
    let mut buf = Vec::new();
    f.take(MAX_FILE + 1).read_to_end(&mut buf).map_err(|_| BAD_FILE)?;
    if buf.len() as u64 > MAX_FILE { return Err(BAD_FILE.into()); }
    let text = decode_lrc(&buf);
    let parsed = parse_lrc(&text);
    if parsed.is_empty() { return Err(BAD_FILE.into()); }
    let timed = parsed.kind() != "plain";
    let (lrc, plain) = if timed { (Some(text), None) } else { (None, Some(text)) };
    Ok(LyricRow {
        status: "ok".into(), source: Some("manual".into()), ext_id: None,
        lrc, plain, pinned: true, fetched: now, ..Default::default()
    })
}

fn pinned_dto(db: &Db, id: &str, row: &LyricRow) -> Result<LyricsDto, String> {
    db.put_lyric(id, row).map_err(|e| e.to_string())?;
    let offset = db.lyric_offset(id).unwrap_or(0);
    let r = Resolved { parsed: parsed_of(row), source: row.source.clone(), status: "ok" };
    Ok(dto_of(id, r, offset))
}

/// Runs blocking lyric work (HTTP, NAS reads) on the blocking pool.
async fn off_thread<T: Send + 'static>(app: AppHandle, f: impl FnOnce(&AppState) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || f(app.state::<AppState>().inner()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn lyrics_get(id: String, app: AppHandle) -> Result<LyricsDto, String> {
    off_thread(app, move |state| {
        let (song, audio) = locate(state, &id)?;
        let names = provider_names(read_settings(&state.db()).netease);
        let boxed = providers_for(&names);
        let refs: Vec<&dyn Provider> = boxed.iter().map(|b| b.as_ref()).collect();
        let still_current = || state.player.current_id().as_deref() == Some(id.as_str());
        let r = resolve(&state.db, &song, audio.as_deref(), &refs, now(), &still_current);
        let offset = state.db().lyric_offset(&id).unwrap_or(0);
        Ok(dto_of(&id, r, offset))
    }).await
}

#[tauri::command]
pub async fn lyrics_search(id: String, title: Option<String>, artist: Option<String>, app: AppHandle) -> Result<Vec<Candidate>, String> {
    off_thread(app, move |state| {
        let (song, names) = {
            let db = state.db();
            (require_song(&db, &id)?, provider_names(read_settings(&db).netease))
        };
        let boxed = providers_for(&names);
        let refs: Vec<&dyn Provider> = boxed.iter().map(|b| b.as_ref()).collect();
        search_with(&refs, &query_of(&song, title, artist))
    }).await
}

#[tauri::command]
pub async fn lyrics_choose(id: String, source: String, ext_id: String, app: AppHandle) -> Result<LyricsDto, String> {
    off_thread(app, move |state| {
        let names = {
            let db = state.db();
            require_song(&db, &id)?;
            provider_names(read_settings(&db).netease)
        };
        let boxed = providers_for(&names);
        let refs: Vec<&dyn Provider> = boxed.iter().map(|b| b.as_ref()).collect();
        let row = fetch_pinned(&refs, &source, &ext_id, now())?;
        pinned_dto(&state.db(), &id, &row)
    }).await
}

#[tauri::command]
pub async fn lyrics_use_file(id: String, path: String, app: AppHandle) -> Result<LyricsDto, String> {
    off_thread(app, move |state| {
        require_song(&state.db(), &id)?;
        let row = row_from_file(Path::new(&path), now())?;
        pinned_dto(&state.db(), &id, &row)
    }).await
}

#[tauri::command(async)]
pub fn lyrics_set_offset(id: String, ms: i32, state: State<'_, AppState>) -> Result<i32, String> {
    let ms = clamp_offset(ms);
    state.db().set_lyric_offset(&id, ms).map_err(|e| e.to_string())?;
    Ok(ms)
}

#[tauri::command(async)]
pub fn lyrics_settings(state: State<'_, AppState>) -> LyricsSettings {
    read_settings(&state.db())
}

#[tauri::command(async)]
pub fn lyrics_set_settings(netease: bool, trans: bool, state: State<'_, AppState>) {
    let db = state.db();
    let _ = db.set_setting("lyrics_netease", if netease { "true" } else { "false" });
    let _ = db.set_setting("lyrics_trans", if trans { "true" } else { "false" });
}

fn cover_color_of(state: &AppState, id: &str) -> Option<[u8; 3]> {
    let (song, root_path) = {
        let db = state.db();
        let song = db.song(id).ok().flatten()?;
        let root = db.roots().ok()?.into_iter().find(|r| r.id == song.root_id)?;
        (song, root.path)
    };
    let online = state.is_online(song.root_id);
    let jpg = crate::cover::cached_cover(&state.data_dir, &root_path, &song, online)?;
    crate::cover::dominant_color(&jpg)
}

#[tauri::command]
pub async fn cover_color(id: String, app: AppHandle) -> Option<[u8; 3]> {
    off_thread(app, move |state| Ok(cover_color_of(state, &id))).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::online::{NetError, Raw};
    use crate::lyrics::Parsed;

    struct Fake { name: &'static str, search: Result<usize, ()>, fetch: Result<Option<&'static str>, ()> }
    impl Provider for Fake {
        fn name(&self) -> &'static str { self.name }
        fn best(&self, _: &Query) -> Result<Option<Raw>, NetError> { Ok(None) }
        fn search(&self, _: &Query) -> Result<Vec<Candidate>, NetError> {
            let n = self.search.map_err(|_| NetError)?;
            Ok((0..n).map(|i| Candidate { source: self.name.into(), ext_id: i.to_string(), title: "晴天".into(),
                artist: "周杰伦".into(), album: "".into(), dur: 1.0, synced: true }).collect())
        }
        fn fetch(&self, _: &str) -> Result<Option<Raw>, NetError> {
            match self.fetch { Err(()) => Err(NetError), Ok(None) => Ok(None),
                Ok(Some(t)) => Ok(Some(Raw { lrc: Some(t.into()), ..Default::default() })) }
        }
    }
    fn fake(name: &'static str, s: Result<usize, ()>, f: Result<Option<&'static str>, ()>) -> Fake { Fake { name, search: s, fetch: f } }
    fn q() -> Query { Query { title: "A".into(), artist: "B".into(), album: "".into(), duration: 1.0 } }

    #[test] fn offset_is_clamped() {
        assert_eq!(clamp_offset(40000), 30000);
        assert_eq!(clamp_offset(-40000), -30000);
        assert_eq!(clamp_offset(123), 123);
    }
    #[test] fn providers_respect_netease_setting() {
        assert_eq!(provider_names(true), vec!["lrclib", "netease"]);
        assert_eq!(provider_names(false), vec!["lrclib"]);
    }
    #[test] fn dto_has_kind_and_offset() {
        let parsed = parse_lrc("[00:01.00]hello\n[00:02.00]world");
        let d = dto_of("s1", Resolved { parsed, source: Some("file".into()), status: "ok" }, -250);
        assert_eq!((d.kind.as_str(), d.offset, d.lines.len(), d.status.as_str()), ("line", -250, 2, "ok"));
        let e = dto_of("s1", Resolved { parsed: Parsed::default(), source: None, status: "none" }, 0);
        assert_eq!((e.kind.as_str(), e.status.as_str()), ("plain", "none"));
    }
    #[test] fn search_merges_and_tolerates_partial_failure() {
        let a = fake("lrclib", Ok(2), Ok(None));
        let b = fake("netease", Ok(1), Ok(None));
        let r = search_with(&[&a, &b], &q()).unwrap();
        assert_eq!(r.iter().map(|c| c.source.as_str()).collect::<Vec<_>>(), vec!["lrclib", "lrclib", "netease"]);
        let bad = fake("lrclib", Err(()), Ok(None));
        assert_eq!(search_with(&[&bad, &b], &q()).unwrap().len(), 1);
        let bad2 = fake("netease", Err(()), Ok(None));
        assert_eq!(search_with(&[&bad, &bad2], &q()), Err("lyrics.netError".into()));
    }
    #[test] fn cleared_search_field_stays_blank() {
        let song = SongRow { id: "s1".into(), title: "晴天".into(), artist: "周杰伦".into(), album: "B".into(), duration: 200.0,
            root_id: 1, file_id: 1, rel: "a.mp3".into(), ext: "mp3".into(), codec: "mp3".into(), bitrate: 0, size: 0,
            cover: 0, track: 1, start_ms: 0, end_ms: None, mtime: 0, added: None };
        let a = query_of(&song, Some("雨天".into()), Some("  ".into()));
        assert_eq!((a.title.as_str(), a.artist.as_str()), ("雨天", ""));
        let b = query_of(&song, None, None);
        assert_eq!((b.title.as_str(), b.artist.as_str()), ("晴天", "周杰伦"));
    }
    #[test] fn search_with_nothing_to_search_asks_nobody() {
        let bad = fake("lrclib", Err(()), Ok(None));
        let blank = Query { title: " ".into(), artist: "".into(), album: "".into(), duration: 1.0 };
        assert_eq!(search_with(&[&bad], &blank), Ok(vec![]));
    }
    #[test] fn choose_errors_and_pins() {
        let ok = fake("lrclib", Ok(0), Ok(Some("[00:01.00]hi")));
        let row = fetch_pinned(&[&ok], "lrclib", "9", 5).unwrap();
        assert!(row.pinned && row.status == "ok" && row.ext_id.as_deref() == Some("9") && row.source.as_deref() == Some("lrclib"));
        assert_eq!(fetch_pinned(&[&ok], "netease", "9", 5), Err("lyrics.netError".into()));
        let net = fake("lrclib", Ok(0), Err(()));
        assert_eq!(fetch_pinned(&[&net], "lrclib", "9", 5), Err("lyrics.netError".into()));
        let none = fake("lrclib", Ok(0), Ok(None));
        assert_eq!(fetch_pinned(&[&none], "lrclib", "9", 5), Err("lyrics.notFound".into()));
        let empty = fake("lrclib", Ok(0), Ok(Some("  \n ")));
        assert_eq!(fetch_pinned(&[&empty], "lrclib", "9", 5), Err("lyrics.notFound".into()));
    }
    #[test] fn use_file_reads_only_and_classifies() {
        let dir = tempfile::tempdir().unwrap();
        let timed = dir.path().join("a.lrc");
        std::fs::write(&timed, "[00:01.00]hi").unwrap();
        let r = row_from_file(&timed, 1).unwrap();
        assert!(r.pinned && r.lrc.is_some() && r.plain.is_none() && r.source.as_deref() == Some("manual"));
        let plain = dir.path().join("b.txt");
        std::fs::write(&plain, "just words\nmore").unwrap();
        let r = row_from_file(&plain, 1).unwrap();
        assert!(r.lrc.is_none() && r.plain.is_some());
        let empty = dir.path().join("c.lrc");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(row_from_file(&empty, 1), Err("lyrics.badFile".into()));
        assert_eq!(row_from_file(&dir.path().join("missing.lrc"), 1), Err("lyrics.badFile".into()));
        let big = dir.path().join("d.lrc");
        std::fs::write(&big, vec![b'a'; 2 * 1024 * 1024 + 1]).unwrap();
        assert_eq!(row_from_file(&big, 1), Err("lyrics.badFile".into()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4);
    }
    #[test] fn settings_default_true_and_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(read_settings(&db), LyricsSettings { netease: true, trans: true });
        db.set_setting("lyrics_netease", "false").unwrap();
        assert_eq!(read_settings(&db), LyricsSettings { netease: false, trans: true });
    }
}
