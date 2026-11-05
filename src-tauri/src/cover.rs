//! Cover thumbnails: extraction, resizing, on-disk cache and the `cover` URI-scheme protocol.

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

/// Cached thumbnail `covers/{file_id}.jpg`; reused when at least as new as the source image.
pub fn cached_cover(data_dir: &Path, root_path: &Path, song: &SongRow) -> Option<Vec<u8>> {
    if !matches!(song.cover, 1 | 2) { return None; }
    let cache = data_dir.join("covers").join(format!("{}.jpg", song.file_id));
    // cheap freshness check before decoding anything
    let src_path = if song.cover == 1 {
        song_file(root_path, song)
    } else {
        crate::scan::dir_cover(song_file(root_path, song).parent()?)?
    };
    let smt = std::fs::metadata(&src_path).and_then(|m| m.modified()).ok();
    let cmt = std::fs::metadata(&cache).and_then(|m| m.modified()).ok();
    if let (Some(s), Some(c)) = (smt, cmt) {
        if c >= s {
            if let Ok(b) = std::fs::read(&cache) { return Some(b); }
        }
    }
    let (src_bytes, _) = source_image(root_path, song)?;
    let jpg = thumbnail(&src_bytes, THUMB_MAX)?;
    let dir = data_dir.join("covers");
    if std::fs::create_dir_all(&dir).is_ok() {
        // atomic publish: unique temp file, then rename over the final path
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("{}.jpg.{}-{}.tmp", song.file_id, std::process::id(), n));
        if std::fs::write(&tmp, &jpg).is_ok() {
            if std::fs::rename(&tmp, &cache).is_err() { let _ = std::fs::remove_file(&tmp); }
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Some(jpg)
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
    cached_cover(&state.data_dir, Path::new(&root_path), &song)
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
    fn song(cover: u8) -> SongRow {
        SongRow { id: "s".into(), root_id: 1, file_id: 7, rel: "a/b.flac".into(), ext: "flac".into(), codec: "flac".into(),
            bitrate: 0, size: 0, cover, title: "".into(), artist: "".into(), album: "".into(), track: 1,
            start_ms: 0, end_ms: None, duration: 1.0 }
    }

    #[test] fn thumbnail_shrinks_long_side() { assert_eq!(dims(&thumbnail(&png(1000, 800), 360).unwrap()), (360, 288)); }
    #[test] fn thumbnail_keeps_small_images() { assert_eq!(dims(&thumbnail(&png(200, 200), 360).unwrap()), (200, 200)); }
    #[test] fn bad_image_returns_none() { assert!(thumbnail(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 360).is_none()); }
    #[test] fn cached_cover_reuses_cache() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a")).unwrap();
        std::fs::write(root.path().join("a").join("cover.png"), png(500, 500)).unwrap();
        let s = song(2);
        let first = cached_cover(data.path(), root.path(), &s).unwrap();
        let cf = data.path().join("covers").join("7.jpg");
        let m1 = std::fs::metadata(&cf).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = cached_cover(data.path(), root.path(), &s).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::metadata(&cf).unwrap().modified().unwrap(), m1);
    }
    #[test] fn folder_cover_used_when_pc_is_2() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a")).unwrap();
        std::fs::write(root.path().join("a").join("cover.png"), png(800, 400)).unwrap();
        let out = cached_cover(data.path(), root.path(), &song(2)).unwrap();
        assert_eq!(dims(&out), (360, 180));
    }
    #[test] fn no_cover_returns_none() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        assert!(cached_cover(data.path(), root.path(), &song(0)).is_none());
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
                    let b = cached_cover(data.path(), root.path(), &s).unwrap();
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
