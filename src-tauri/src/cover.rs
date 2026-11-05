//! Cover thumbnails: extraction, resizing, on-disk cache and the `cover` URI-scheme protocol.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::db::SongRow;

pub const THUMB_MAX: u32 = 360;

/// Absolute path of the song's audio file.
fn song_file(root_path: &Path, song: &SongRow) -> PathBuf {
    let mut p = root_path.to_path_buf();
    for part in song.rel.split('/').filter(|c| !c.is_empty()) { p.push(part); }
    p
}

/// Raw cover image for a song (embedded or folder image) and the file it came from.
pub fn source_image(root_path: &Path, song: &SongRow) -> Option<(Vec<u8>, PathBuf)> {
    let file = song_file(root_path, song);
    match song.cover {
        1 => crate::tags::read_cover(&file).map(|(b, _)| (b, file)),
        2 => {
            let img = crate::scan::dir_cover(file.parent()?)?;
            std::fs::read(&img).ok().map(|b| (b, img))
        }
        _ => None,
    }
}

/// Decodes `bytes` and shrinks the long side to `max` (never enlarges); JPEG quality 85.
pub fn thumbnail(bytes: &[u8], max: u32) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let img = if img.width().max(img.height()) > max {
        img.resize(max, max, image::imageops::FilterType::CatmullRom)
    } else { img };
    let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    rgb.write_with_encoder(enc).ok()?;
    Some(out)
}

/// Cache file name for an audio file's cover: `{sha1(root, rel)[..16]}-{size}-{mtime}.jpg`.
/// Stable across database ids (SQLite reuses `files.id`), shared by the CUE tracks of one file,
/// and changed whenever the audio file itself changes.
pub fn cache_name(root_path: &str, rel: &str, size: u64, mtime: i64) -> String {
    format!("{}-{}-{}.jpg", crate::db::song_id(root_path, rel, 0), size, mtime)
}

/// Cached thumbnail `covers/{cache_name}.jpg`. For an online root it is reused when at least as
/// new as the source image, otherwise regenerated. For an offline root the source is never
/// touched (a dead SMB share blocks on every stat): an existing cache file is served as is.
pub fn cached_cover(data_dir: &Path, root_path: &str, song: &SongRow, online: bool) -> Option<Vec<u8>> {
    if !matches!(song.cover, 1 | 2) { return None; }
    let name = cache_name(root_path, &song.rel, song.size, song.mtime);
    let dir = data_dir.join("covers");
    let cache = dir.join(&name);
    if !online {
        return std::fs::read(&cache).ok();
    }
    let root = Path::new(root_path);
    // cheap freshness check before decoding anything
    let src_path = if song.cover == 1 {
        song_file(root, song)
    } else {
        crate::scan::dir_cover(song_file(root, song).parent()?)?
    };
    let smt = std::fs::metadata(&src_path).and_then(|m| m.modified()).ok();
    let cmt = std::fs::metadata(&cache).and_then(|m| m.modified()).ok();
    if let (Some(s), Some(c)) = (smt, cmt) {
        if c >= s {
            if let Ok(b) = std::fs::read(&cache) { return Some(b); }
        }
    }
    let (src_bytes, _) = source_image(root, song)?;
    let jpg = thumbnail(&src_bytes, THUMB_MAX)?;
    if std::fs::create_dir_all(&dir).is_ok() {
        // atomic publish: unique temp file, then rename over the final path
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("{name}.{}-{}.tmp", std::process::id(), n));
        if std::fs::write(&tmp, &jpg).is_ok() {
            if std::fs::rename(&tmp, &cache).is_err() { let _ = std::fs::remove_file(&tmp); }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Some(jpg)
}

/// Cache names of every file in the library that has a cover.
pub fn keep_set(db: &crate::db::Db) -> crate::db::Result<HashSet<String>> {
    Ok(db.cover_files()?.into_iter().map(|(root, rel, size, mtime)| cache_name(&root, &rel, size, mtime)).collect())
}

/// Deletes cached thumbnails not in `keep` (removed/changed files, removed folders, and the
/// old `{file_id}.jpg` scheme). In-flight `.tmp` files are left alone. Returns the count removed.
pub fn prune_cache(data_dir: &Path, keep: &HashSet<String>) -> usize {
    let Ok(rd) = std::fs::read_dir(data_dir.join("covers")) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".jpg") && !keep.contains(&name) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// Old-scheme names: `{digits}.jpg` (keyed by the reusable `files.id`).
fn is_legacy(name: &str) -> bool {
    name.strip_suffix(".jpg").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Removes old-scheme cache files; cheap, local, safe to run at every startup.
pub fn purge_legacy(data_dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(data_dir.join("covers")) else { return 0 };
    rd.flatten()
        .filter(|e| is_legacy(&e.file_name().to_string_lossy()))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Prunes the cache against the database. The DB lock is held only while listing files.
pub fn prune_with_db(state: &crate::state::AppState) -> usize {
    let keep = match keep_set(&state.db()) {
        Ok(k) => k,
        Err(_) => return 0, // never prune against an unreadable library
    };
    prune_cache(&state.data_dir, &keep)
}

/// Handles one `cover://` request; runs on a worker thread.
pub fn respond(app: &tauri::AppHandle, uri_path: &str) -> Option<Vec<u8>> {
    use tauri::Manager;
    let id = uri_path.split('?').next()?.trim_start_matches('/');
    if id.is_empty() { return None; }
    let state = app.state::<crate::state::AppState>();
    let (song, root_path) = {
        let db = state.db();
        let song = db.song(id).ok().flatten()?;
        let root = db.roots().ok()?.into_iter().find(|r| r.id == song.root_id)?;
        (song, root.path)
    }; // DB lock released before any file IO
    let online = state.is_online(song.root_id);
    cached_cover(&state.data_dir, &root_path, &song, online)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }
    fn dims(b: &[u8]) -> (u32, u32) {
        let i = image::load_from_memory_with_format(b, image::ImageFormat::Jpeg).unwrap();
        (i.width(), i.height())
    }
    fn song_at(cover: u8, rel: &str) -> SongRow {
        SongRow { id: "s".into(), root_id: 1, file_id: 7, rel: rel.into(), ext: "flac".into(), codec: "flac".into(),
            bitrate: 0, size: 1234, cover, title: "".into(), artist: "".into(), album: "".into(), track: 1,
            start_ms: 0, end_ms: None, duration: 1.0, mtime: 1_700_000_000 }
    }
    fn song(cover: u8) -> SongRow { song_at(cover, "a/b.flac") }
    fn rs(p: &Path) -> &str { p.to_str().unwrap() }
    fn put_cover(root: &Path, dir: &str, bytes: &[u8]) {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(root.join(dir).join("cover.png"), bytes).unwrap();
    }

    #[test] fn thumbnail_shrinks_long_side() { assert_eq!(dims(&thumbnail(&png(1000, 800), 360).unwrap()), (360, 288)); }
    #[test] fn thumbnail_keeps_small_images() { assert_eq!(dims(&thumbnail(&png(200, 200), 360).unwrap()), (200, 200)); }
    #[test] fn bad_image_returns_none() { assert!(thumbnail(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 360).is_none()); }
    #[test] fn cache_name_is_stable_and_shared_by_cue_tracks() {
        let n = cache_name("D:/m", "a/b.flac", 1234, 99);
        assert_eq!(n, format!("{}-1234-99.jpg", crate::db::song_id("D:/m", "a/b.flac", 0)));
        assert_eq!(n, cache_name("D:/m", "a\\b.flac", 1234, 99));
        assert_ne!(n, cache_name("D:/m", "a/b.flac", 1235, 99));
        assert_ne!(n, cache_name("D:/m", "a/b.flac", 1234, 100));
        assert_ne!(n, cache_name("E:/m", "a/b.flac", 1234, 99));
        // two CUE tracks of one file (start 0 and 3000 ms) use the same SongRow file fields
        let mut t2 = song(2);
        t2.start_ms = 3000;
        assert_eq!(cache_name("D:/m", &t2.rel, t2.size, t2.mtime), cache_name("D:/m", &song(2).rel, song(2).size, song(2).mtime));
    }
    #[test] fn cached_cover_reuses_cache() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        put_cover(root.path(), "a", &png(500, 500));
        let s = song(2);
        let first = cached_cover(data.path(), rs(root.path()), &s, true).unwrap();
        let cf = data.path().join("covers").join(cache_name(rs(root.path()), &s.rel, s.size, s.mtime));
        let m1 = std::fs::metadata(&cf).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = cached_cover(data.path(), rs(root.path()), &s, true).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::metadata(&cf).unwrap().modified().unwrap(), m1);
    }
    #[test] fn different_file_at_reused_file_id_gets_its_own_cover() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        put_cover(root.path(), "a", &png(500, 500));
        put_cover(root.path(), "c", &png(800, 400));
        // an old-scheme cache file for the same file id must not be picked up either
        std::fs::create_dir_all(data.path().join("covers")).unwrap();
        std::fs::write(data.path().join("covers").join("7.jpg"), thumbnail(&png(100, 300), 360).unwrap()).unwrap();
        let first = cached_cover(data.path(), rs(root.path()), &song_at(2, "a/b.flac"), true).unwrap();
        assert_eq!(dims(&first), (360, 360));
        // the folder was removed and another file now has file_id 7
        let other = cached_cover(data.path(), rs(root.path()), &song_at(2, "c/d.flac"), true).unwrap();
        assert_eq!(dims(&other), (360, 180));
        // same rel under another root path is another file too
        let root2 = tempfile::tempdir().unwrap();
        put_cover(root2.path(), "a", &png(300, 600));
        assert_eq!(dims(&cached_cover(data.path(), rs(root2.path()), &song_at(2, "a/b.flac"), true).unwrap()), (180, 360));
    }
    #[test] fn stale_cache_regenerates_when_source_is_newer() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        put_cover(root.path(), "a", &png(500, 500));
        let s = song(2);
        assert_eq!(dims(&cached_cover(data.path(), rs(root.path()), &s, true).unwrap()), (360, 360));
        // folder image replaced (the audio file, hence the cache name, is unchanged)
        let img = root.path().join("a").join("cover.png");
        std::fs::write(&img, png(800, 400)).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&img).unwrap().set_modified(later).unwrap();
        assert_eq!(dims(&cached_cover(data.path(), rs(root.path()), &s, true).unwrap()), (360, 180));
        // and the regenerated file is what the cache now holds
        let cf = data.path().join("covers").join(cache_name(rs(root.path()), &s.rel, s.size, s.mtime));
        assert_eq!(dims(&std::fs::read(cf).unwrap()), (360, 180));
    }
    #[test] fn offline_root_serves_cache_without_source() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        put_cover(root.path(), "a", &png(500, 500));
        let s = song(2);
        let root_path = rs(root.path()).to_string();
        let online = cached_cover(data.path(), &root_path, &s, true).unwrap();
        drop(root); // the share is gone
        assert_eq!(cached_cover(data.path(), &root_path, &s, false), Some(online));
        // no cache for this file: nothing to serve, and nothing is generated
        assert_eq!(cached_cover(data.path(), &root_path, &song_at(2, "x/y.flac"), false), None);
        assert_eq!(cached_cover(data.path(), &root_path, &song(0), false), None);
    }
    #[test] fn prune_keeps_only_listed_names_and_tmp() {
        let data = tempfile::tempdir().unwrap();
        let dir = data.path().join("covers");
        std::fs::create_dir_all(&dir).unwrap();
        let keep = cache_name("D:/m", "a.flac", 1, 2);
        for n in ["7.jpg", "123.jpg", keep.as_str(), "deadbeefdeadbeef-1-2.jpg", "x.jpg.1-2.tmp"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        assert_eq!(purge_legacy(data.path()), 2);
        assert!(!dir.join("7.jpg").exists() && dir.join(&keep).exists());
        assert_eq!(prune_cache(data.path(), &HashSet::from([keep.clone()])), 1);
        let mut left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, vec![keep, "x.jpg.1-2.tmp".to_string()]);
        let none = tempfile::tempdir().unwrap();
        assert_eq!((purge_legacy(none.path()), prune_cache(none.path(), &HashSet::new())), (0, 0));
    }
    #[test] fn keep_set_lists_files_with_covers() {
        let mut db = crate::db::Db::open_in_memory().unwrap();
        let r = db.add_root("D:/m").unwrap();
        let m = |rel: &str, cover: u8| crate::db::FileMeta { rel: rel.into(), ext: "flac".into(), size: 5, mtime: 9,
            codec: "flac".into(), bitrate: 0, duration: 1.0, cover, cue_sig: None };
        db.replace_file(r.id, &m("a.flac", 1), &[]).unwrap();
        db.replace_file(r.id, &m("b.flac", 0), &[]).unwrap();
        db.replace_file(r.id, &m("c.flac", 2), &[]).unwrap();
        assert_eq!(keep_set(&db).unwrap(), HashSet::from([cache_name("D:/m", "a.flac", 5, 9), cache_name("D:/m", "c.flac", 5, 9)]));
    }
    #[test] fn folder_cover_used_when_pc_is_2() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        put_cover(root.path(), "a", &png(800, 400));
        let out = cached_cover(data.path(), rs(root.path()), &song(2), true).unwrap();
        assert_eq!(dims(&out), (360, 180));
    }
    #[test] fn no_cover_returns_none() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        assert!(cached_cover(data.path(), rs(root.path()), &song(0), true).is_none());
    }

    #[test] fn concurrent_cache_writes_are_valid_and_leave_no_tmp() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a")).unwrap();
        std::fs::write(root.path().join("a").join("cover.png"), png(1200, 1200)).unwrap();
        let s = song(2);
        std::thread::scope(|sc| {
            for _ in 0..8 {
                sc.spawn(|| for _ in 0..5 {
                    let b = cached_cover(data.path(), rs(root.path()), &s, true).unwrap();
                    assert_eq!(dims(&b), (360, 360));
                });
            }
        });
        let left: Vec<_> = std::fs::read_dir(data.path().join("covers")).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(left.is_empty());
    }
    #[test] fn decompression_bomb_header_returns_none() {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        v.extend_from_slice(&100000u32.to_be_bytes());
        v.extend_from_slice(&100000u32.to_be_bytes());
        v.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        let t = std::time::Instant::now();
        assert!(thumbnail(&v, 360).is_none());
        assert!(t.elapsed().as_secs() < 2);
    }
}
