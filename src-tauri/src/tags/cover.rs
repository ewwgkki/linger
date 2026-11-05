//! Embedded cover extraction (port of read_cover; Ogg added).

use std::path::Path;

use super::{flac, id3, mp4, ogg, wav, Src};

pub(super) fn read_cover(path: &Path) -> Option<(Vec<u8>, &'static str)> {
    let mut src = Src::open(path)?;
    let head = src.read_at(0, 10);
    let mut id3len = 0u64;
    let mut found = None;
    if head.len() == 10 && head.starts_with(b"ID3") {
        id3len = id3::tag_len(&head);
        found = id3::id3_cover(&src.read_at(0, id3len.min(32 << 20) as usize));
    }
    if found.is_none() {
        let magic = src.read_at(id3len, 4);
        if magic == b"fLaC" {
            found = flac::flac_cover(&mut src, id3len + 4).ok().flatten();
        } else if src.read_at(4, 4) == b"ftyp" {
            found = mp4::mp4_cover(&mut src);
        } else if magic == b"RIFF" {
            found = wav::wav_cover(&mut src);
        } else if magic == b"OggS" {
            found = ogg::ogg_cover(&mut src, id3len);
        }
    }
    let (mime, data) = found?;
    if data.len() <= 100 {
        return None;
    }
    let mime = sniff(&data).or_else(|| declared(&mime))?;
    Some((data, mime))
}

fn sniff(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(b"\xFF\xD8") {
        Some("image/jpeg")
    } else if d.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if d.starts_with(b"GIF") {
        Some("image/gif")
    } else {
        None
    }
}

/// Declared mime when the data was not recognised; PHP keeps any `image/*` value.
/// The return type needs a static string, so unknown image subtypes become octet-stream.
fn declared(mime: &str) -> Option<&'static str> {
    let m = mime.to_ascii_lowercase();
    if !m.starts_with("image/") {
        return None;
    }
    Some(match m.as_str() {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "image/jpeg",
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/bmp" | "image/x-ms-bmp" => "image/bmp",
        "image/webp" => "image/webp",
        _ => "application/octet-stream",
    })
}
