//! Spotify import: parse export files, match tracks to the library, create playlists and
//! report what could not be matched. Parsing and matching run without the db lock.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::db::{Db, SongRow};
use crate::import::matcher::{LibSong, Matcher};
use crate::import::{decode_text, parse_zip, playlist_name_from_file, spotify_json, csv, ParseError, SpPlaylist};
use crate::state::AppState;

use super::playlists::PlaylistsChanged;

const MAX_FILE: u64 = 50 * 1024 * 1024;
const LOSSLESS: [&str; 4] = ["pcm", "flac", "alac", "ape"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Missing { pub t: String, pub a: String, pub b: String }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportedDto {
    pub source: String,
    pub name: String,
    pub id: Option<i64>,
    pub total: usize,
    pub matched: usize,
    pub missing: Vec<Missing>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailedDto { pub source: String, pub reason: String }

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ImportReport { pub playlists: Vec<ImportedDto>, pub failed: Vec<FailedDto> }

/// "x" -> "x (2)" -> "x (3)" until the name is free.
pub fn unique_name(existing: &[String], want: &str) -> String {
    if !existing.iter().any(|n| n == want) {
        return want.to_string();
    }
    (2..)
        .map(|i| format!("{want} ({i})"))
        .find(|n| !existing.iter().any(|e| e == n))
        .unwrap()
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Reads and parses every file; no db access.
fn parse_files(paths: &[PathBuf], report: &mut ImportReport) -> Vec<(String, SpPlaylist)> {
    let mut out = Vec::new();
    let mut fail = |source: String, reason: &str| report.failed.push(FailedDto { source, reason: reason.into() });
    for p in paths {
        let name = file_name(p);
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "csv" | "zip" | "json") {
            fail(name, "import.unsupported");
            continue;
        }
        match std::fs::metadata(p) {
            Ok(m) if m.len() > MAX_FILE => { fail(name, "import.tooLarge"); continue; }
            Ok(_) => {}
            Err(_) => { fail(name, "import.unreadable"); continue; }
        }
        let Ok(bytes) = std::fs::read(p) else { fail(name, "import.unreadable"); continue };
        match ext.as_str() {
            _ if bytes.iter().all(|b| b.is_ascii_whitespace()) => fail(name, ParseError::Empty.key()),
            "csv" => {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                match csv::parse_csv(&playlist_name_from_file(&stem), &decode_text(&bytes)) {
                    Ok(pl) => out.push((name, pl)),
                    Err(e) => fail(name, e.key()),
                }
            }
            "json" => match spotify_json::parse_spotify_json(&decode_text(&bytes)) {
                Ok(v) if v.is_empty() => fail(name, ParseError::Empty.key()),
                Ok(v) => out.extend(v.into_iter().map(|pl| (name.clone(), pl))),
                Err(e) => fail(name, e.key()),
            },
            _ => {
                for (entry, res) in parse_zip(&bytes) {
                    let base = entry.rsplit(['/', '\\']).next().unwrap_or(&entry);
                    if entry.starts_with("__MACOSX/") || base.starts_with("._") {
                        continue;
                    }
                    let source = if entry.is_empty() { name.clone() } else { format!("{name}/{entry}") };
                    match res {
                        Ok(pl) => out.push((source, pl)),
                        Err(e) => fail(source, e.key()),
                    }
                }
            }
        }
    }
    out
}

struct Matched { source: String, name: String, ids: Vec<String>, total: usize, matched: usize, missing: Vec<Missing> }

/// Matches against a song snapshot; no db access.
fn match_all(songs: &[SongRow], parsed: Vec<(String, SpPlaylist)>) -> Vec<Matched> {
    let lib = songs
        .iter()
        .map(|s| LibSong {
            id: &s.id,
            title: &s.title,
            artist: &s.artist,
            duration: s.duration,
            quality: if LOSSLESS.contains(&s.codec.as_str()) { 9999 } else { s.bitrate },
        })
        .collect();
    let matcher = Matcher::new(lib);
    parsed
        .into_iter()
        .map(|(source, pl)| {
            let mut ids = Vec::new();
            let mut missing = Vec::new();
            for t in &pl.tracks {
                match matcher.find(t) {
                    Some(id) => ids.push(id.to_string()),
                    None => missing.push(Missing { t: t.title.clone(), a: t.artists.join(", "), b: t.album.clone() }),
                }
            }
            Matched { source, name: pl.name, total: pl.tracks.len(), matched: ids.len(), ids, missing }
        })
        .collect()
}

/// Creates the playlists (one per matched list) and fills the report.
fn apply(db: &mut Db, matched: Vec<Matched>, now: i64, report: &mut ImportReport) {
    let mut names: Vec<String> = db.playlists().unwrap_or_default().into_iter().map(|p| p.name).collect();
    for m in matched {
        let mut name = m.name.clone();
        let mut id = None;
        if m.matched > 0 {
            name = unique_name(&names, &m.name);
            if let Ok(pid) = db.create_playlist(&name, now) {
                let _ = db.playlist_add(pid, &m.ids, now);
                names.push(name.clone());
                id = Some(pid);
            }
        }
        report.playlists.push(ImportedDto {
            source: m.source, name, id, total: m.total, matched: m.matched, missing: m.missing,
        });
    }
}

pub fn import_files(db: &mut Db, paths: &[PathBuf], now: i64) -> ImportReport {
    let mut report = ImportReport::default();
    let parsed = parse_files(paths, &mut report);
    let songs = db.all_songs().unwrap_or_default();
    let matched = match_all(&songs, parsed);
    apply(db, matched, now, &mut report);
    report
}

#[tauri::command(async)]
pub fn import_spotify(paths: Vec<String>, app: AppHandle, state: State<'_, AppState>) -> ImportReport {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut report = ImportReport::default();
    let parsed = parse_files(&paths, &mut report);
    let songs = state.db().all_songs().unwrap_or_default();
    let matched = match_all(&songs, parsed);
    apply(&mut state.db(), matched, now, &mut report);
    let _ = app.emit("playlists://changed", PlaylistsChanged { id: None, favorites: false });
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{song_id, FileMeta, NewSong};

    fn lib(db: &mut Db) -> Vec<String> {
        let r = db.add_root("D:/m").unwrap();
        let m = FileMeta { rel: "x.flac".into(), ext: "flac".into(), size: 7, mtime: 1, codec: "flac".into(),
            bitrate: 900, duration: 10.0, cover: 0, cue_sig: None };
        let s = |t: &str, a: &str, st: u64| NewSong { id: song_id("D:/m", "x.flac", st), title: t.into(),
            artist: a.into(), album: "B".into(), track: 1, start_ms: st, end_ms: None, duration: 3.0 };
        db.replace_file(r.id, &m, &[s("晴天", "周杰伦", 0), s("Yesterday", "The Beatles", 3000)]).unwrap();
        vec![song_id("D:/m", "x.flac", 0), song_id("D:/m", "x.flac", 3000)]
    }

    const CSV: &str = "Track Name,Artist Name(s),Album Name,Duration (ms)\n\
        Yesterday,The Beatles,Help,3000\n晴天,周杰伦,叶惠美,3000\nNope,Nobody,Nothing,1000\n";

    fn write(dir: &tempfile::TempDir, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn unique_name_suffixes() {
        let ex = vec!["x".to_string(), "x (2)".to_string()];
        assert_eq!(unique_name(&ex, "x"), "x (3)");
        assert_eq!(unique_name(&ex, "y"), "y");
    }

    #[test]
    fn import_csv_creates_playlist_in_order() {
        let mut db = Db::open_in_memory().unwrap();
        let ids = lib(&mut db);
        let d = tempfile::tempdir().unwrap();
        let p = write(&d, "My_Mix.csv", CSV.as_bytes());
        let r = import_files(&mut db, &[p], 5);
        assert!(r.failed.is_empty());
        let pl = &r.playlists[0];
        assert_eq!((pl.name.as_str(), pl.source.as_str(), pl.total, pl.matched), ("My Mix", "My_Mix.csv", 3, 2));
        assert_eq!(pl.missing, vec![Missing { t: "Nope".into(), a: "Nobody".into(), b: "Nothing".into() }]);
        let items: Vec<String> = db.playlist_items(pl.id.unwrap()).unwrap().into_iter().map(|i| i.song_id).collect();
        assert_eq!(items, vec![ids[1].clone(), ids[0].clone()]);
    }

    #[test]
    fn zero_match_creates_nothing() {
        let mut db = Db::open_in_memory().unwrap();
        let d = tempfile::tempdir().unwrap();
        let p = write(&d, "a.csv", CSV.as_bytes());
        let r = import_files(&mut db, &[p], 5);
        assert_eq!(r.playlists.len(), 1);
        assert_eq!(r.playlists[0].id, None);
        assert_eq!(r.playlists[0].matched, 0);
        assert!(db.playlists().unwrap().is_empty());
    }

    #[test]
    fn bad_file_reported_others_imported() {
        let mut db = Db::open_in_memory().unwrap();
        lib(&mut db);
        let d = tempfile::tempdir().unwrap();
        let ps = [write(&d, "good.csv", CSV.as_bytes()), write(&d, "empty.csv", b""),
            write(&d, "junk.json", b"{"), write(&d, "song.mp3", b"x")];
        let r = import_files(&mut db, &ps, 5);
        assert_eq!(r.playlists.len(), 1);
        let f: Vec<(&str, &str)> = r.failed.iter().map(|f| (f.source.as_str(), f.reason.as_str())).collect();
        assert_eq!(f, vec![("empty.csv", "import.empty"), ("junk.json", "import.badJson"), ("song.mp3", "import.unsupported")]);
    }

    #[test]
    fn same_name_twice_gets_suffix() {
        let mut db = Db::open_in_memory().unwrap();
        lib(&mut db);
        let d = tempfile::tempdir().unwrap();
        let p = write(&d, "My_Mix.csv", CSV.as_bytes());
        import_files(&mut db, &[p.clone()], 5);
        let r = import_files(&mut db, &[p], 6);
        assert_eq!(r.playlists[0].name, "My Mix (2)");
    }

    #[test]
    fn zip_entries_sourced_and_macosx_dropped() {
        use std::io::Write;
        let mut db = Db::open_in_memory().unwrap();
        lib(&mut db);
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            for n in ["a_b.csv", "__MACOSX/._a_b.csv", "d/._c.csv", "bad.csv"] {
                w.start_file(n, o).unwrap();
                w.write_all(if n == "a_b.csv" { CSV.as_bytes() } else { b"junk" }).unwrap();
            }
            w.finish().unwrap();
        }
        let d = tempfile::tempdir().unwrap();
        let p = write(&d, "e.zip", &buf);
        let r = import_files(&mut db, &[p], 5);
        assert_eq!(r.playlists.len(), 1);
        assert_eq!(r.playlists[0].source, "e.zip/a_b.csv");
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].source, "e.zip/bad.csv");
    }
}
