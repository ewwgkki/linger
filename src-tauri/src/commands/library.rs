//! Library commands and events for the frontend.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::db::{Db, Root, SongRow};
use crate::scan::{root_online, scan_shared, Skipped};
use crate::state::AppState;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RootDto {
    pub id: i64,
    pub path: String,
    pub online: bool,
    pub count: usize,
}

/// Field names match the web version's library JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SongDto {
    pub id: String,
    pub t: String,
    pub a: String,
    pub b: String,
    pub n: u32,
    /// seconds
    pub s: f64,
    pub c: String,
    pub f: String,
    pub br: u32,
    pub z: u64,
    pub pc: u8,
    pub r: i64,
    pub p: String,
    pub st: u64,
    pub av: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryDto {
    pub roots: Vec<RootDto>,
    pub songs: Vec<SongDto>,
    pub scanned_at: Option<i64>,
    pub ffmpeg: bool,
}

/// Trims trailing separators ("D:\m\" == "D:\m") except on a drive root ("D:\")
/// or a UNC share root ("\\nas\music\"); separators are otherwise kept as given.
pub fn normalize_root(p: &str) -> String {
    let p = p.trim();
    let is_sep = |c: char| c == '\\' || c == '/';
    let t = p.trim_end_matches(is_sep);
    if t.is_empty() {
        // "/" (or "\\") stays as given; blank input becomes ""
        return if p.is_empty() { String::new() } else { p[..1].to_string() };
    }
    let b = t.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return format!("{t}\\");
    }
    if p.starts_with("\\\\") || p.starts_with("//") {
        let parts = t[2..].split(is_sep).filter(|s| !s.is_empty()).count();
        if parts <= 2 {
            let sep = if p.starts_with("\\\\") { '\\' } else { '/' };
            return format!("{t}{sep}");
        }
    }
    t.to_string()
}

pub fn roots_dto(db: &Db, online: &HashSet<i64>) -> crate::db::Result<Vec<RootDto>> {
    let counts = db.root_song_counts()?;
    Ok(db
        .roots()?
        .into_iter()
        .map(|r| RootDto {
            online: online.contains(&r.id),
            count: counts.get(&r.id).copied().unwrap_or(0),
            id: r.id,
            path: r.path,
        })
        .collect())
}

fn song_dto(s: SongRow, online: &HashSet<i64>) -> SongDto {
    SongDto {
        av: online.contains(&s.root_id),
        id: s.id,
        t: s.title,
        a: s.artist,
        b: s.album,
        n: s.track,
        s: s.duration,
        c: s.codec,
        f: s.ext,
        br: s.bitrate,
        z: s.size,
        pc: s.cover,
        r: s.root_id,
        p: s.rel,
        st: s.start_ms,
    }
}

pub fn library_dto(db: &Db, online: &HashSet<i64>, ffmpeg: bool) -> crate::db::Result<LibraryDto> {
    let mut songs: Vec<SongDto> = db.all_songs()?.into_iter().map(|s| song_dto(s, online)).collect();
    songs.sort_by(|x, y| (&x.a, &x.b, x.n, &x.t).cmp(&(&y.a, &y.b, y.n, &y.t)));
    Ok(LibraryDto {
        roots: roots_dto(db, online)?,
        songs,
        scanned_at: db.setting("scanned_at")?.and_then(|v| v.parse().ok()),
        ffmpeg,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanFinished {
    pub count: usize,
    pub skipped: Vec<Skipped>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Returns straight from the database; never touches the network. Async: building and
/// serializing a large library must not block the main thread.
#[tauri::command(async)]
pub fn get_library(state: State<'_, AppState>) -> Result<LibraryDto, String> {
    let online = state.online();
    let ffmpeg = crate::ffmpeg::find_ffmpeg().is_some();
    library_dto(&state.db(), &online, ffmpeg).map_err(err)
}

#[tauri::command(async)]
pub fn add_roots(paths: Vec<String>, state: State<'_, AppState>) -> Result<Vec<RootDto>, String> {
    let db = state.db();
    let mut online = state.online();
    for p in paths.iter().map(|p| normalize_root(p)).filter(|p| !p.is_empty()) {
        // optimistic until the next scan probes it
        online.insert(db.add_root(&p).map_err(err)?.id);
    }
    state.set_online(online.clone());
    roots_dto(&db, &online).map_err(err)
}

/// Async: the cascade delete of a large folder and the cover-cache prune take a while.
#[tauri::command(async)]
pub fn remove_root(id: i64, state: State<'_, AppState>) -> Result<Vec<RootDto>, String> {
    let dto = {
        let db = state.db();
        db.remove_root(id).map_err(err)?;
        state.mark_offline(id);
        roots_dto(&db, &state.online()).map_err(err)?
    };
    crate::cover::prune_with_db(&state);
    Ok(dto)
}

/// Starts a background probe + incremental scan; during a running scan, queues one more pass.
#[tauri::command]
pub fn start_scan(app: AppHandle) {
    spawn_scan(app);
}

/// Probes every root in parallel (each capped at 3 s).
pub fn probe_roots(roots: &[Root]) -> HashSet<i64> {
    let handles: Vec<_> = roots
        .iter()
        .map(|r| {
            let (id, path) = (r.id, r.path.clone());
            std::thread::spawn(move || (id, root_online(Path::new(&path), PROBE_TIMEOUT)))
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).filter(|(_, ok)| *ok).map(|(id, _)| id).collect()
}

/// One scan thread at a time. A request made while a scan runs queues exactly one more pass,
/// so a folder added mid-scan (or during the startup probe) is still scanned.
#[derive(Debug, Default)]
pub struct ScanGate {
    running: AtomicBool,
    again: AtomicBool,
}

impl ScanGate {
    /// Returns true when the caller must start a scan thread.
    pub fn request(&self) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            self.again.store(true, Ordering::SeqCst);
            false
        } else {
            true
        }
    }
    /// Called by the scan thread after each pass; true means run another pass.
    pub fn finish(&self) -> bool {
        if self.again.swap(false, Ordering::SeqCst) {
            return true;
        }
        self.running.store(false, Ordering::SeqCst);
        // a request that arrived between the check and the store
        self.again.swap(false, Ordering::SeqCst) && !self.running.swap(true, Ordering::SeqCst)
    }
    /// The scan thread could not start or panicked.
    pub fn abort(&self) {
        self.again.store(false, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
    }
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Releases the gate if the scan thread panics (normal exits go through `finish`).
struct PanicGuard<'a>(&'a AppState);
impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.scan.abort();
        }
    }
}

fn scan_pass(app: &AppHandle, state: &AppState) {
    let roots = state.db().roots().unwrap_or_default();
    let online = probe_roots(&roots);
    state.set_online(online.clone());
    if let Ok(dto) = roots_dto(&state.db(), &online) {
        let _ = app.emit("library://roots", dto);
    }
    let rep = scan_shared(&state.db, &roots, &online, &mut |p| {
        let _ = app.emit("scan://progress", p);
    });
    // drop thumbnails of files that changed, vanished or belong to removed folders
    crate::cover::prune_with_db(state);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = state.db().set_setting("scanned_at", &now.to_string());
    let _ = app.emit("scan://finished", ScanFinished { count: rep.count, skipped: rep.skipped });
}

pub fn spawn_scan(app: AppHandle) {
    if !app.state::<AppState>().scan.request() {
        return;
    }
    let app2 = app.clone();
    let spawned = std::thread::Builder::new()
        .name("linger-scan".into())
        .spawn(move || {
            let app = app2;
            let state = app.state::<AppState>();
            let _guard = PanicGuard(&state);
            loop {
                scan_pass(&app, &state);
                if !state.scan.finish() {
                    break;
                }
            }
        });
    if spawned.is_err() {
        app.state::<AppState>().scan.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{song_id, FileMeta, NewSong};

    #[test]
    fn scan_gate_queues_one_more_pass() {
        let g = ScanGate::default();
        assert!(g.request(), "first request starts a thread");
        assert!(!g.request(), "second request is queued");
        assert!(!g.request(), "requests coalesce");
        assert!(g.finish(), "queued request runs another pass");
        assert!(!g.finish(), "then the thread stops");
        assert!(!g.running());
        assert!(g.request(), "idle again: a new request starts a thread");
        g.abort();
        assert!(!g.running());
        assert!(g.request());
    }

    #[test]
    fn normalize_root_cases() {
        let cases = [
            (r"D:\m\", r"D:\m"),
            (r"D:\m", r"D:\m"),
            (r"  D:\m\\ ", r"D:\m"),
            (r"D:\", r"D:\"),
            (r"D:", r"D:\"),
            (r"\\nas\music\", r"\\nas\music\"),
            (r"\\nas\music", r"\\nas\music\"),
            (r"\\nas\music\Kai\", r"\\nas\music\Kai"),
            ("/", "/"),
            ("/mnt/m/", "/mnt/m"),
            ("   ", ""),
        ];
        for (i, o) in cases {
            assert_eq!(normalize_root(i), o, "{i:?}");
        }
    }

    #[test]
    fn dtos() {
        let mut db = Db::open_in_memory().unwrap();
        let r1 = db.add_root("D:/m").unwrap();
        let r2 = db.add_root("E:/n").unwrap();
        let m = |rel: &str| FileMeta { rel: rel.into(), ext: "flac".into(), size: 7, mtime: 1, codec: "flac".into(),
            bitrate: 900, duration: 10.0, cover: 2, cue_sig: None };
        let s = |root: &str, rel: &str, a: &str, n: u32, st: u64| NewSong { id: song_id(root, rel, st), title: format!("t{n}"),
            artist: a.into(), album: "b".into(), track: n, start_ms: st, end_ms: None, duration: 2.5 };
        db.replace_file(r1.id, &m("x.flac"), &[s("D:/m", "x.flac", "Z", 2, 0)]).unwrap();
        db.replace_file(r2.id, &m("y.flac"), &[s("E:/n", "y.flac", "A", 1, 0), s("E:/n", "y.flac", "A", 2, 3000)]).unwrap();
        db.set_setting("scanned_at", "1700000000").unwrap();
        let online = HashSet::from([r1.id]);

        let roots = roots_dto(&db, &online).unwrap();
        assert_eq!(roots, vec![
            RootDto { id: r1.id, path: "D:/m".into(), online: true, count: 1 },
            RootDto { id: r2.id, path: "E:/n".into(), online: false, count: 2 },
        ]);

        let lib = library_dto(&db, &online, true).unwrap();
        assert_eq!(lib.scanned_at, Some(1_700_000_000));
        assert!(lib.ffmpeg);
        assert_eq!(lib.songs.len(), 3);
        // sorted like the web version: artist, album, track, title
        assert_eq!(lib.songs.iter().map(|s| (s.a.as_str(), s.n)).collect::<Vec<_>>(), [("A", 1), ("A", 2), ("Z", 2)]);
        let x = &lib.songs[1];
        assert_eq!((x.st, x.s, x.r, x.p.as_str(), x.f.as_str(), x.c.as_str(), x.br, x.z, x.pc, x.av),
                   (3000, 2.5, r2.id, "y.flac", "flac", "flac", 900, 7, 2, false));
        assert!(lib.songs[2].av);

        let json = serde_json::to_value(&lib.songs[0]).unwrap();
        for k in ["id", "t", "a", "b", "n", "s", "c", "f", "br", "z", "pc", "r", "p", "st", "av"] {
            assert!(json.get(k).is_some(), "{k}");
        }
    }
}
