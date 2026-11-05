//! Incremental library scanner: walks the music roots, splits CUE images and
//! writes files/songs to the database.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;

use crate::cue::{parse_cue, parse_tracklist_txt, resolve, CueSong};
use crate::db::{song_id, Db, FileMeta, FileSig, NewSong, Root};
use crate::derive::{derive, RawTags};
use crate::tags::read_audio_info;

pub const SKIP_DIRS: &[&str] = &["@eaDir", "#recycle", "#snapshot", "$RECYCLE.BIN", "System Volume Information"];
pub const AUDIO_EXT: &[&str] = &["mp3", "mp2", "flac", "m4a", "mp4", "aac", "wav", "wma", "ape", "ogg", "opus"];
const COVER_NAMES: &[&str] = &["封面", "cover", "folder", "front", "albumartlarge", "albumart"];
const COVER_EXT: &[&str] = &["jpg", "jpeg", "png"];
/// Re-probe timeout when a root seems to vanish during a scan.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skipped {
    pub path: String,
    /// i18n key, e.g. "skip.notaudio"
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanReport {
    /// songs in the whole library after the scan (offline roots included)
    pub count: usize,
    /// files whose tags were actually read
    pub read: usize,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// `is_dir` on a helper thread; a probe that has not answered within `timeout`
/// counts as offline (unreachable SMB hosts can block for tens of seconds).
pub fn root_online(path: &Path, timeout: Duration) -> bool {
    let p = path.to_path_buf();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("linger-probe".into())
        .spawn(move || {
            let _ = tx.send(p.is_dir());
        })
        .is_ok();
    spawned && rx.recv_timeout(timeout).unwrap_or(false)
}

fn split_ext(name: &str) -> (&str, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], name[i + 1..].to_ascii_lowercase()),
        _ => (name, String::new()),
    }
}

/// Folder cover image (`封面.jpg`, `cover.png`, ...; case-insensitive), best-ranked name first.
pub fn dir_cover(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(usize, String)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        let (stem, ext) = split_ext(&name);
        if !COVER_EXT.contains(&ext.as_str()) {
            continue;
        }
        let Some(rank) = COVER_NAMES.iter().position(|n| *n == stem.to_lowercase()) else { continue };
        if best.as_ref().map_or(true, |(r, n)| (rank, &name) < (*r, n)) {
            best = Some((rank, name));
        }
    }
    best.map(|(_, n)| dir.join(n))
}

/// Full scan holding `&mut Db` (tests, CLI).
pub fn scan(db: &mut Db, roots: &[Root], online: &HashSet<i64>, progress: &mut dyn FnMut(Progress)) -> ScanReport {
    let cell = RefCell::new(db);
    scan_with(&mut |f| f(&mut cell.borrow_mut()), roots, online, progress)
}

/// Same as [`scan`], but locks the shared database only for each individual read/write,
/// so the UI can query the library while a scan runs.
pub fn scan_shared(db: &Mutex<Db>, roots: &[Root], online: &HashSet<i64>, progress: &mut dyn FnMut(Progress)) -> ScanReport {
    scan_with(
        &mut |f| f(&mut db.lock().unwrap_or_else(|e| e.into_inner())),
        roots,
        online,
        progress,
    )
}

type WithDb<'a> = dyn FnMut(&mut dyn FnMut(&mut Db)) + 'a;

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    size: u64,
    mtime: i64,
}
#[derive(Debug, Default)]
struct DirFiles {
    audio: Vec<Entry>,
    cues: Vec<Entry>,
    txts: Vec<String>,
}
struct Walk {
    /// rel dir ("" = root, '/'-separated) -> its files
    dirs: BTreeMap<String, DirFiles>,
    /// rel dirs that could not be listed ("" = the root itself)
    failed: Vec<String>,
}

fn join_rel(base: &str, name: &str) -> String {
    if base.is_empty() { name.to_string() } else { format!("{base}/{name}") }
}

fn abs_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

fn mtime_of(m: &std::fs::Metadata) -> i64 {
    match m.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        },
        Err(_) => 0,
    }
}

fn walk(root: &Path) -> Walk {
    let mut w = Walk { dirs: BTreeMap::new(), failed: vec![] };
    let mut stack = vec![String::new()];
    while let Some(sub) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(abs_path(root, &sub)) else {
            w.failed.push(sub);
            continue;
        };
        let mut df = DirFiles::default();
        let mut partial = false;
        for e in rd {
            // a listing cut short (SMB drop) must not look complete, or the rest gets deleted
            let Ok(e) = e else {
                partial = true;
                break;
            };
            let Ok(name) = e.file_name().into_string() else { continue };
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else {
                partial = true;
                continue;
            };
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(join_rel(&sub, &name));
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let (_, ext) = split_ext(&name);
            let is_audio = AUDIO_EXT.contains(&ext.as_str());
            if ext == "txt" {
                df.txts.push(name);
            } else if is_audio || ext == "cue" {
                let Ok(m) = e.metadata() else {
                    partial = true;
                    continue;
                };
                let ent = Entry { name, size: m.len(), mtime: mtime_of(&m) };
                if is_audio { df.audio.push(ent) } else { df.cues.push(ent) }
            }
        }
        df.audio.sort_by(|a, b| a.name.cmp(&b.name));
        df.cues.sort_by(|a, b| a.name.cmp(&b.name));
        df.txts.sort();
        if partial {
            w.failed.push(sub.clone());
        }
        w.dirs.insert(sub, df);
    }
    w
}

fn cue_sig(e: &Entry) -> String {
    format!("{}:{}", e.size, e.mtime)
}

/// Audio file name -> (CUE tracks for it, cue_sig).
type CuePlan = HashMap<String, (Vec<CueSong>, String)>;

/// Matches each parseable CUE in the directory to its audio file(s):
/// exact FILE name (case-insensitive), else same stem with another extension,
/// else - for a single-FILE CUE - the directory's only unclaimed audio file.
fn plan_cues(root: &Path, dir_rel: &str, folder: &str, df: &DirFiles) -> CuePlan {
    let mut plan = CuePlan::new();
    if df.cues.is_empty() || df.audio.is_empty() {
        return plan;
    }
    let mut lists: Option<Vec<Vec<(u32, String)>>> = None;
    for c in &df.cues {
        let Ok(bytes) = std::fs::read(abs_path(root, &join_rel(dir_rel, &c.name))) else { continue };
        let Some(sheet) = parse_cue(&bytes) else { continue };
        let lists = lists.get_or_insert_with(|| {
            df.txts
                .iter()
                .filter_map(|t| std::fs::read(abs_path(root, &join_rel(dir_rel, t))).ok())
                .map(|b| parse_tracklist_txt(&b))
                .filter(|l| !l.is_empty())
                .collect()
        });
        let ntracks: usize = sheet.files.iter().map(|f| f.tracks.len()).sum();
        // longest track list that does not exceed the CUE (exact match preferred)
        let list = lists.iter().filter(|l| l.len() <= ntracks).max_by_key(|l| l.len()).cloned().unwrap_or_default();
        let songs = resolve(&sheet, folder, &list);
        let sig = cue_sig(c);
        let unclaimed = |plan: &CuePlan| -> Vec<&Entry> {
            df.audio.iter().filter(|a| !plan.contains_key(&a.name)).collect()
        };
        for f in &sheet.files {
            let want = f.name.rsplit(['/', '\\']).next().unwrap_or(&f.name).to_lowercase();
            let want_stem = split_ext(&want).0.to_string();
            let free = unclaimed(&plan);
            let hit = free
                .iter()
                .find(|a| a.name.to_lowercase() == want)
                .or_else(|| free.iter().find(|a| split_ext(&a.name.to_lowercase()).0 == want_stem))
                .or_else(|| (sheet.files.len() == 1 && free.len() == 1).then(|| &free[0]))
                .map(|a| a.name.clone());
            if let Some(name) = hit {
                let mine: Vec<CueSong> = songs.iter().filter(|s| s.file_name == f.name).cloned().collect();
                if !mine.is_empty() {
                    plan.insert(name, (mine, sig.clone()));
                }
            }
        }
    }
    plan
}

fn build_songs(root: &Root, rel: &str, info: &crate::tags::AudioInfo, cue: Option<&Vec<CueSong>>) -> Vec<NewSong> {
    if let Some(cs) = cue {
        return cs
            .iter()
            .map(|c| {
                let start = c.start_ms as f64 / 1000.0;
                let duration = match c.end_ms {
                    Some(e) => e.saturating_sub(c.start_ms) as f64 / 1000.0,
                    None => info.duration - start,
                };
                NewSong {
                    id: song_id(&root.path, rel, c.start_ms),
                    title: c.title.clone(),
                    artist: c.artist.clone(),
                    album: c.album.clone(),
                    track: c.number,
                    start_ms: c.start_ms,
                    end_ms: c.end_ms,
                    duration: duration.max(0.0),
                }
            })
            .collect();
    }
    let raw = RawTags {
        title: info.title.clone(),
        artist: info.artist.clone(),
        album: info.album.clone(),
        albumartist: info.albumartist.clone(),
        track: info.track,
    };
    let d = derive(rel, &raw, info.duration);
    vec![NewSong {
        id: song_id(&root.path, rel, 0),
        title: d.title,
        artist: d.artist,
        album: d.album,
        track: d.track,
        start_ms: 0,
        end_ms: None,
        duration: info.duration,
    }]
}

/// Stored files of a root that were not seen, excluding anything under a folder
/// that could not be (fully) listed; nothing at all if the root itself failed.
fn gone_files(sigs: &HashMap<String, FileSig>, seen: &HashSet<String>, failed: &[String]) -> Vec<String> {
    if failed.iter().any(|f| f.is_empty()) {
        return vec![];
    }
    let prefixes: Vec<String> = failed.iter().map(|f| format!("{f}/")).collect();
    sigs.keys()
        .filter(|rel| !seen.contains(*rel))
        .filter(|rel| !prefixes.iter().any(|p| rel.starts_with(p.as_str())))
        .cloned()
        .collect()
}

fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn scan_with(with: &mut WithDb, roots: &[Root], online: &HashSet<i64>, progress: &mut dyn FnMut(Progress)) -> ScanReport {
    let mut rep = ScanReport::default();
    let walks: Vec<(&Root, Walk)> = roots
        .iter()
        .filter(|r| online.contains(&r.id))
        .map(|r| (r, walk(Path::new(&r.path))))
        .collect();
    let total: usize = walks.iter().flat_map(|(_, w)| w.dirs.values()).map(|d| d.audio.len()).sum();
    let mut done = 0usize;
    let mut last_emit = Instant::now();
    progress(Progress { done, total });

    for (root, w) in &walks {
        let root_path = Path::new(&root.path);
        for f in &w.failed {
            rep.skipped.push(Skipped { path: display(&abs_path(root_path, f)), why: "skip.unreadable".into() });
        }
        let mut sigs: HashMap<String, FileSig> = HashMap::new();
        with(&mut |db| sigs = db.file_sigs(root.id).unwrap_or_default());
        let mut seen: HashSet<String> = HashSet::new();
        let mut root_lost = false;

        for (dir_rel, df) in &w.dirs {
            let folder = match dir_rel.rsplit('/').next() {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => root_path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            };
            let cue_sigs: HashSet<String> = df.cues.iter().map(cue_sig).collect();
            // Parse CUEs only when some file here may have changed.
            let trivially_same = |a: &Entry, rel: &str| {
                sigs.get(rel).is_some_and(|s| {
                    s.size == a.size
                        && s.mtime == a.mtime
                        && match &s.cue_sig {
                            None => df.cues.is_empty(),
                            Some(c) => cue_sigs.contains(c),
                        }
                })
            };
            let plan = if df.audio.iter().all(|a| trivially_same(a, &join_rel(dir_rel, &a.name))) {
                CuePlan::new()
            } else {
                plan_cues(root_path, dir_rel, &folder, df)
            };
            let mut cover: Option<Option<PathBuf>> = None;

            for a in &df.audio {
                let rel = join_rel(dir_rel, &a.name);
                let cue = plan.get(&a.name);
                let unchanged = if plan.is_empty() && trivially_same(a, &rel) {
                    true
                } else {
                    sigs.get(&rel).is_some_and(|s| {
                        s.size == a.size && s.mtime == a.mtime && s.cue_sig.as_ref() == cue.map(|c| &c.1)
                    })
                };
                let abs = abs_path(root_path, &rel);
                if unchanged || root_lost {
                    // root_lost: the share went away mid-scan; keep everything as it is
                    seen.insert(rel);
                } else if std::fs::File::open(&abs).is_err() {
                    // transient failure (SMB hiccup, locked file): keep the old row and its old
                    // sig so the next scan retries, instead of storing an empty song
                    rep.skipped.push(Skipped { path: display(&abs), why: "skip.unreadable".into() });
                    seen.insert(rel);
                    if !root_online(root_path, PROBE_TIMEOUT) {
                        root_lost = true;
                    }
                } else {
                    let info = read_audio_info(&abs);
                    rep.read += 1;
                    if info.codec == "notaudio" {
                        rep.skipped.push(Skipped { path: display(&abs), why: "skip.notaudio".into() });
                    } else {
                        let pc = if info.pic {
                            1
                        } else if cover.get_or_insert_with(|| dir_cover(&abs_path(root_path, dir_rel))).is_some() {
                            2
                        } else {
                            0
                        };
                        let meta = FileMeta {
                            rel: rel.clone(),
                            ext: split_ext(&a.name).1,
                            size: a.size,
                            mtime: a.mtime,
                            codec: info.codec.clone(),
                            bitrate: info.bitrate,
                            duration: info.duration,
                            cover: pc,
                            cue_sig: cue.map(|c| c.1.clone()),
                        };
                        let songs = build_songs(root, &rel, &info, cue.map(|c| &c.0));
                        let mut ok = true;
                        with(&mut |db| ok = db.replace_file(root.id, &meta, &songs).is_ok());
                        if !ok {
                            rep.skipped.push(Skipped { path: display(&abs), why: "skip.dberror".into() });
                        }
                        // on failure the previous row (if any) stays
                        seen.insert(rel);
                    }
                }
                done += 1;
                if done % 20 == 0 || last_emit.elapsed() >= Duration::from_millis(200) {
                    progress(Progress { done, total });
                    last_emit = Instant::now();
                }
            }
        }

        // Forget files no longer present - unless the root vanished mid-scan.
        let gone = gone_files(&sigs, &seen, &w.failed);
        if !gone.is_empty() && !root_lost && root_online(root_path, PROBE_TIMEOUT) {
            with(&mut |db| {
                let _ = db.delete_files(root.id, &gone);
            });
        }
    }
    progress(Progress { done, total });
    with(&mut |db| rep.count = db.song_count().unwrap_or(0));
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::testutil::{gbk, id3v2_23, latin1_frame, mp3_cbr_frames, wav_pcm};
    use std::fs;
    use std::time::Instant;

    fn mp3(title: &str) -> Vec<u8> {
        [id3v2_23(&[("TIT2", latin1_frame(title.as_bytes()))]), mp3_cbr_frames(40)].concat()
    }
    fn put(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn setup(dir: &Path) -> (Db, Vec<Root>, HashSet<i64>) {
        let db = Db::open_in_memory().unwrap();
        let r = db.add_root(dir.to_str().unwrap()).unwrap();
        let online = HashSet::from([r.id]);
        (db, vec![r], online)
    }
    fn run(db: &mut Db, roots: &[Root], online: &HashSet<i64>) -> ScanReport {
        scan(db, roots, online, &mut |_| {})
    }

    const XW: &str = "示例歌手《晴天集》首版 典藏[整轨WAV]";
    fn cue_album(root: &Path, folder: &str, wav_name: &str) {
        let cue = "PERFORMER \"未知艺术家\"\r\nTITLE \"未知标题\"\r\nFILE \"未知艺术家 - 未知标题.wav\" WAVE\r\n\
            \x20 TRACK 01 AUDIO\r\n    TITLE \"Track01\"\r\n    INDEX 01 00:00:00\r\n\
            \x20 TRACK 02 AUDIO\r\n    TITLE \"Track02\"\r\n    INDEX 01 00:03:00\r\n\
            \x20 TRACK 03 AUDIO\r\n    TITLE \"Track03\"\r\n    INDEX 01 00:06:00\r\n";
        put(root, &format!("{folder}/未知艺术家 - 未知标题.cue"), &gbk(cue));
        put(root, &format!("{folder}/专辑曲目.txt"), &gbk("专辑曲目:\r\n01 甲\r\n02 乙\r\n03 丙\r\n"));
        put(root, &format!("{folder}/{wav_name}"), &wav_pcm(10.0, 8000, &[]));
    }

    #[test]
    fn scan_basic_tree_and_skip_dirs() {
        let t = tempfile::tempdir().unwrap();
        for rel in ["a/x.mp3", "@eaDir/y.mp3", "#recycle/z.mp3", ".hidden/w.mp3", "a/notes.doc"] {
            put(t.path(), rel, &mp3("x"));
        }
        let (mut db, roots, online) = setup(t.path());
        let mut last = None;
        let rep = scan(&mut db, &roots, &online, &mut |p| last = Some(p));
        assert_eq!(rep.count, 1);
        assert_eq!(rep.read, 1);
        assert!(rep.skipped.is_empty(), "{:?}", rep.skipped);
        assert_eq!(last, Some(Progress { done: 1, total: 1 }));
        let s = db.all_songs().unwrap();
        assert_eq!(s[0].rel, "a/x.mp3");
        assert_eq!(s[0].title, "x");
        assert_eq!(s[0].codec, "mp3");
    }

    #[test]
    fn scan_skips_fake_mp3_with_reason() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "fake.mp3", &[&b"MZ\x90\0"[..], &[0u8; 1000]].concat());
        put(t.path(), "ok.mp3", &mp3("ok"));
        let (mut db, roots, online) = setup(t.path());
        let rep = run(&mut db, &roots, &online);
        assert_eq!(rep.count, 1);
        assert_eq!(rep.skipped.len(), 1);
        assert_eq!(rep.skipped[0].why, "skip.notaudio");
        assert!(rep.skipped[0].path.ends_with("fake.mp3"));
        assert!(db.all_songs().unwrap().iter().all(|s| s.rel != "fake.mp3"));
    }

    #[test]
    fn scan_splits_cue_album() {
        let t = tempfile::tempdir().unwrap();
        cue_album(t.path(), XW, "未知艺术家 - 未知标题.wav");
        let (mut db, roots, online) = setup(t.path());
        let rep = run(&mut db, &roots, &online);
        assert_eq!(rep.count, 3);
        let s = db.all_songs().unwrap();
        let titles: Vec<_> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["甲", "乙", "丙"]);
        assert!(s.iter().all(|s| s.artist == "示例歌手" && s.album == "晴天集"));
        assert_eq!(s.iter().map(|s| s.track).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!((s[1].start_ms, s[1].end_ms), (3000, Some(6000)));
        assert!((s[0].duration - 3.0).abs() < 0.01, "{}", s[0].duration);
        assert!((s[2].duration - 4.0).abs() < 0.01, "{}", s[2].duration);
        assert_eq!(s[2].end_ms, None);
        assert_ne!(s[0].id, s[1].id);
    }

    #[test]
    fn scan_cue_matches_renamed_single_image() {
        // CUE says "未知艺术家 - 未知标题.wav" but the only audio file was renamed
        let t = tempfile::tempdir().unwrap();
        cue_album(t.path(), XW, "晴天集.wav");
        let (mut db, roots, online) = setup(t.path());
        assert_eq!(run(&mut db, &roots, &online).count, 3);
    }

    #[test]
    fn scan_cue_matches_other_extension() {
        let t = tempfile::tempdir().unwrap();
        cue_album(t.path(), XW, "未知艺术家 - 未知标题.WAV");
        put(t.path(), &format!("{XW}/bonus.mp3"), &mp3("bonus"));
        let (mut db, roots, online) = setup(t.path());
        assert_eq!(run(&mut db, &roots, &online).count, 4);
    }

    #[test]
    fn rescan_unchanged_reads_nothing() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "a/x.mp3", &mp3("x"));
        put(t.path(), "b.mp3", &mp3("b"));
        cue_album(t.path(), XW, "未知艺术家 - 未知标题.wav");
        let (mut db, roots, online) = setup(t.path());
        let first = run(&mut db, &roots, &online);
        assert_eq!((first.count, first.read), (5, 3));
        let second = run(&mut db, &roots, &online);
        assert_eq!((second.count, second.read), (5, 0));
    }

    #[test]
    fn rescan_detects_change_and_deletion() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "a/x.mp3", &mp3("x"));
        put(t.path(), "b.mp3", &mp3("b"));
        put(t.path(), "c.mp3", &mp3("c"));
        let (mut db, roots, online) = setup(t.path());
        run(&mut db, &roots, &online);
        put(t.path(), "b.mp3", &mp3("b-changed"));
        fs::remove_file(t.path().join("c.mp3")).unwrap();
        let rep = run(&mut db, &roots, &online);
        assert_eq!((rep.count, rep.read), (2, 1));
        let s = db.all_songs().unwrap();
        assert!(s.iter().any(|s| s.title == "b-changed"));
        assert!(s.iter().all(|s| s.rel != "c.mp3"));
    }

    #[test]
    fn rescan_detects_new_cue() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), &format!("{XW}/未知艺术家 - 未知标题.wav"), &wav_pcm(10.0, 8000, &[]));
        let (mut db, roots, online) = setup(t.path());
        assert_eq!(run(&mut db, &roots, &online).count, 1);
        cue_album(t.path(), XW, "未知艺术家 - 未知标题.wav");
        let rep = run(&mut db, &roots, &online);
        assert_eq!((rep.count, rep.read), (3, 1));
    }

    #[test]
    fn offline_root_keeps_songs() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "a/x.mp3", &mp3("x"));
        let (mut db, roots, online) = setup(t.path());
        run(&mut db, &roots, &online);
        fs::remove_file(t.path().join("a/x.mp3")).unwrap();
        let rep = run(&mut db, &roots, &HashSet::new());
        assert_eq!((rep.count, rep.read), (1, 0));
        assert_eq!(db.all_songs().unwrap().len(), 1);
    }

    #[test]
    fn cover_flag() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "a/x.mp3", &mp3("x"));
        put(t.path(), "a/Cover.JPEG", b"img");
        put(t.path(), "b/y.mp3", &mp3("y"));
        let (mut db, roots, online) = setup(t.path());
        run(&mut db, &roots, &online);
        let s = db.all_songs().unwrap();
        let get = |rel: &str| s.iter().find(|s| s.rel == rel).unwrap().cover;
        assert_eq!((get("a/x.mp3"), get("b/y.mp3")), (2, 0));
    }

    #[test]
    fn dir_cover_case_insensitive_priority() {
        let t = tempfile::tempdir().unwrap();
        for n in ["Folder.png", "封面.JPG", "back.jpg", "cover.gif"] {
            put(t.path(), n, b"img");
        }
        assert_eq!(dir_cover(t.path()), Some(t.path().join("封面.JPG")));
        let e = tempfile::tempdir().unwrap();
        assert_eq!(dir_cover(e.path()), None);
    }

    #[test]
    fn scan_shared_works_with_mutex() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "a/x.mp3", &mp3("x"));
        let (db, roots, online) = setup(t.path());
        let m = Mutex::new(db);
        assert_eq!(scan_shared(&m, &roots, &online, &mut |_| {}).count, 1);
    }

    #[test]
    fn rescan_detects_cue_change() {
        let t = tempfile::tempdir().unwrap();
        cue_album(t.path(), XW, "未知艺术家 - 未知标题.wav");
        let (mut db, roots, online) = setup(t.path());
        run(&mut db, &roots, &online);
        let cue = "FILE \"未知艺术家 - 未知标题.wav\" WAVE\r\n  TRACK 01 AUDIO\r\n    TITLE \"新歌一\"\r\n    INDEX 01 00:00:00\r\n\
                   \x20 TRACK 02 AUDIO\r\n    TITLE \"新歌二\"\r\n    INDEX 01 00:05:00\r\n";
        put(t.path(), &format!("{XW}/未知艺术家 - 未知标题.cue"), &gbk(cue));
        let rep = run(&mut db, &roots, &online);
        assert_eq!((rep.count, rep.read), (2, 1));
        let titles: Vec<_> = db.all_songs().unwrap().into_iter().map(|s| s.title).collect();
        assert_eq!(titles, ["新歌一", "新歌二"]);
    }

    #[test]
    fn removing_cue_reverts_to_one_song() {
        let t = tempfile::tempdir().unwrap();
        cue_album(t.path(), XW, "未知艺术家 - 未知标题.wav");
        let (mut db, roots, online) = setup(t.path());
        assert_eq!(run(&mut db, &roots, &online).count, 3);
        fs::remove_file(t.path().join(XW).join("未知艺术家 - 未知标题.cue")).unwrap();
        let rep = run(&mut db, &roots, &online);
        assert_eq!((rep.count, rep.read), (1, 1));
        let s = db.all_songs().unwrap();
        assert_eq!((s[0].start_ms, s[0].end_ms), (0, None));
        assert!((s[0].duration - 10.0).abs() < 0.01);
    }

    #[test]
    fn dberror_keeps_previous_row() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "b.mp3", &mp3("b"));
        let (mut db, roots, online) = setup(t.path());
        run(&mut db, &roots, &online);
        db.conn()
            .execute_batch("CREATE TRIGGER boom BEFORE INSERT ON songs WHEN NEW.title='boom' BEGIN SELECT RAISE(ABORT,'boom'); END;")
            .unwrap();
        put(t.path(), "b.mp3", &mp3("boom"));
        let rep = run(&mut db, &roots, &online);
        assert_eq!(rep.skipped.len(), 1);
        assert_eq!(rep.skipped[0].why, "skip.dberror");
        let s = db.all_songs().unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].title, "b");
    }

    #[cfg(windows)]
    #[test]
    fn open_failure_keeps_previous_row() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        put(t.path(), "a.mp3", &mp3("a"));
        put(t.path(), "b.mp3", &mp3("b"));
        let (mut db, roots, online) = setup(t.path());
        run(&mut db, &roots, &online);
        put(t.path(), "b.mp3", &mp3("b-changed-and-longer"));
        // exclusive handle: any other open fails with a sharing violation
        let lock = fs::OpenOptions::new().read(true).share_mode(0).open(t.path().join("b.mp3")).unwrap();
        let rep = run(&mut db, &roots, &online);
        assert_eq!(rep.skipped.iter().map(|s| s.why.as_str()).collect::<Vec<_>>(), ["skip.unreadable"]);
        let s = db.all_songs().unwrap();
        assert_eq!(s.len(), 2);
        assert!(s.iter().any(|s| s.title == "b" && s.duration > 0.0));
        drop(lock);
        // once readable again, the change is picked up (the old sig was kept)
        let rep = run(&mut db, &roots, &online);
        assert_eq!(rep.read, 1);
        assert!(db.all_songs().unwrap().iter().any(|s| s.title == "b-changed-and-longer"));
    }

    #[test]
    fn gone_files_respects_failed_dirs() {
        let sig = FileSig { id: 1, size: 1, mtime: 1, cue_sig: None };
        let sigs: HashMap<String, FileSig> = ["a/x.mp3", "a/b/y.mp3", "ab/z.mp3", "c.mp3", "d.mp3"]
            .into_iter()
            .map(|r| (r.to_string(), sig.clone()))
            .collect();
        let seen = HashSet::from(["d.mp3".to_string()]);
        let mut gone = gone_files(&sigs, &seen, &["a".to_string()]);
        gone.sort();
        assert_eq!(gone, ["ab/z.mp3", "c.mp3"]);
        assert!(gone_files(&sigs, &seen, &[String::new()]).is_empty());
    }

    #[test]
    fn root_online_local() {
        let t = tempfile::tempdir().unwrap();
        assert!(root_online(t.path(), Duration::from_secs(3)));
        assert!(!root_online(&t.path().join("missing"), Duration::from_secs(3)));
    }

    #[test]
    fn root_online_times_out_for_unreachable_unc() {
        let t = Instant::now();
        assert!(!root_online(Path::new(r"\\linger-no-such-host\share"), Duration::from_secs(3)));
        assert!(t.elapsed() < Duration::from_secs(4));
    }
}
