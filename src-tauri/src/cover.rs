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
    let img = image::load_from_memory(bytes).ok()?;
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
    let cache = data_dir.join("covers").join(format!("{}.jpg", song.file_id));
    let (src_bytes, src_path) = match song.cover {
        1 | 2 => {
            // cheap freshness check before decoding anything
            let src_path = match song.cover {
                1 => song_file(root_path, song),
                _ => crate::scan::dir_cover(song_file(root_path, song).parent()?)?,
            };
            let smt = std::fs::metadata(&src_path).and_then(|m| m.modified()).ok();
            let cmt = std::fs::metadata(&cache).and_then(|m| m.modified()).ok();
            if let (Some(s), Some(c)) = (smt, cmt) {
                if c >= s {
                    if let Ok(b) = std::fs::read(&cache) { return Some(b); }
                }
            }
            let (b, p) = source_image(root_path, song)?;
            (b, p)
        }
        _ => return None,
    };
    let _ = src_path;
    let jpg = thumbnail(&src_bytes, THUMB_MAX)?;
    if std::fs::create_dir_all(data_dir.join("covers")).is_ok() {
        let _ = std::fs::write(&cache, &jpg);
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
}
