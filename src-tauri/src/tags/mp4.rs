//! MP4 / M4A (port of read_mp4, mp4_children, mp4_find, parse_moov, mp4_cover).

use super::{byte, legacy_text, sub, tail, u32be, AudioInfo, Src, Tags};

/// Contents of the top-level `moov` box, if any.
fn find_moov(src: &mut Src) -> Option<Vec<u8>> {
    let size = src.size;
    let mut pos = 0u64;
    while pos + 8 <= size {
        let h = src.read_at(pos, 16);
        if h.len() < 8 {
            return None;
        }
        let mut len = u32be(&h, 0) as u64;
        let typ = &h[4..8];
        let mut hdr = 8u64;
        if len == 1 {
            len = (u32be(&h, 8) as u64) << 32 | u32be(&h, 12) as u64;
            hdr = 16;
        } else if len == 0 {
            len = size - pos;
        }
        if len < 8 {
            return None;
        }
        if typ == b"moov" {
            let n = len.saturating_sub(hdr).min(64 << 20) as usize;
            return Some(src.read_at(pos + hdr, n));
        }
        pos = pos.saturating_add(len);
    }
    None
}

pub(super) fn read_mp4(src: &mut Src, info: &mut AudioInfo) {
    if let Some(moov) = find_moov(src) {
        parse_moov(&moov, info);
    }
}

fn children(d: &[u8], start: usize) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let (mut p, n) = (start, d.len());
    while p + 8 <= n {
        let len = u32be(d, p) as usize;
        if len < 8 || p + len > n {
            break;
        }
        out.push((&d[p + 4..p + 8], &d[p + 8..p + len]));
        p += len;
    }
    out
}

fn find<'a>(d: &'a [u8], path: &[&[u8; 4]], start: usize) -> Option<&'a [u8]> {
    let (name, rest) = path.split_first()?;
    for (t, body) in children(d, start) {
        if t != &name[..] {
            continue;
        }
        if rest.is_empty() {
            return Some(body);
        }
        if let Some(r) = find(body, rest, if t == b"meta" { 4 } else { 0 }) {
            return Some(r);
        }
    }
    None
}

fn parse_moov(moov: &[u8], info: &mut AudioInfo) {
    info.codec = "aac".into();
    if let Some(mvhd) = find(moov, &[b"mvhd"], 0) {
        let (ts, dur) = if byte(mvhd, 0) == 1 {
            (u32be(mvhd, 20), u32be(mvhd, 24) as f64 * 4294967296.0 + u32be(mvhd, 28) as f64)
        } else {
            (u32be(mvhd, 12), u32be(mvhd, 16) as f64)
        };
        if ts != 0 {
            info.duration = dur / ts as f64;
        }
    }
    for (t, trak) in children(moov, 0) {
        if t != b"trak" {
            continue;
        }
        if let Some(stsd) = find(trak, &[b"mdia", b"minf", b"stbl", b"stsd"], 0) {
            if stsd.len() >= 16 {
                match &stsd[12..16] {
                    b"alac" => info.codec = "alac".into(),
                    b"mp4a" => info.codec = "aac".into(),
                    _ => {}
                }
                break;
            }
        }
    }
    let Some(ilst) = find(moov, &[b"udta", b"meta", b"ilst"], 0) else { return };
    let mut tags = Tags::default();
    for (t, body) in children(ilst, 0) {
        if t == b"covr" && body.len() > 100 {
            info.pic = true;
        }
        let key = match t {
            b"\xA9nam" => "title",
            b"\xA9ART" => "artist",
            b"\xA9alb" => "album",
            b"aART" => "albumartist",
            b"trkn" => "track",
            _ => continue,
        };
        let Some(data) = find(body, &[b"data"], 0) else { continue };
        if data.len() < 8 {
            continue;
        }
        let v = tail(data, 8);
        // later atoms of the same kind overwrite earlier ones, as in PHP
        if key == "track" {
            tags.track = Some(if v.len() >= 4 { (v[2] as u32) << 8 | v[3] as u32 } else { 0 });
        } else {
            tags.set(key, legacy_text(v));
        }
    }
    info.merge(tags);
}

/// Port of mp4_cover: first `covr/data` as (mime, data).
pub(super) fn mp4_cover(src: &mut Src) -> Option<(String, Vec<u8>)> {
    let moov = find_moov(src)?;
    let data = find(&moov, &[b"udta", b"meta", b"ilst", b"covr", b"data"], 0)?;
    if data.len() < 8 {
        return None;
    }
    let mime = if u32be(data, 0) == 14 { "image/png" } else { "image/jpeg" };
    Some((mime.to_string(), sub(data, 8, data.len()).to_vec()))
}
