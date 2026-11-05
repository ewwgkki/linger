//! ID3v2 / ID3v1 (port of id3_frames, parse_id3v2, id3_text, parse_id3v1, id3_cover).

use super::{byte, conv_utf16, legacy_text, sub, synchsafe, tail, u32be, Tags, Utf16};
use crate::text::{clean_text, decode_legacy, is_broken};

/// Total length of an ID3v2 tag from its 10-byte header (incl. footer when flagged).
pub(super) fn tag_len(head: &[u8]) -> u64 {
    10 + synchsafe(head, 6) + if byte(head, 5) & 0x10 != 0 { 10 } else { 0 }
}

fn replace_ff00(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        out.push(b[i]);
        i += if b[i] == 0xFF && b.get(i + 1) == Some(&0) { 2 } else { 1 };
    }
    out
}

/// PHP `preg_match('/^[A-Z0-9]{3,4}$/', $id)` ($ also matches before a final "\n").
fn valid_id(id: &[u8]) -> bool {
    let id = id.strip_suffix(b"\n").unwrap_or(id);
    (3..=4).contains(&id.len()) && id.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// All frames of an ID3v2 tag as (id, body).
pub(super) fn id3_frames(tag: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut frames = Vec::new();
    if tag.len() < 10 || !tag.starts_with(b"ID3") {
        return frames;
    }
    let ver = tag[3];
    let flags = tag[5];
    let end = (tag.len() as u64).min(10 + synchsafe(tag, 6)) as usize;
    let mut data = tag[10..end.max(10)].to_vec();
    if flags & 0x80 != 0 && ver < 4 {
        data = replace_ff00(&data);
    }
    let mut pos: u64 = 0;
    if flags & 0x40 != 0 {
        pos = if ver >= 4 { synchsafe(&data, 0) } else { u32be(&data, 0) as u64 + 4 };
    }
    let (id_len, hdr_len) = if ver == 2 { (3u64, 6u64) } else { (4, 10) };
    let len = data.len() as u64;
    while pos + hdr_len <= len {
        let p = pos as usize;
        if !valid_id(sub(&data, p, id_len as usize)) {
            break;
        }
        let fs: u64 = if ver == 2 {
            (byte(&data, p + 3) as u64) << 16 | (byte(&data, p + 4) as u64) << 8 | byte(&data, p + 5) as u64
        } else if ver >= 4 {
            synchsafe(&data, p + 4)
        } else {
            u32be(&data, p + 4) as u64
        };
        let mut body = sub(&data, p + hdr_len as usize, fs.min(len) as usize).to_vec();
        if ver >= 4 && len > pos + 9 && data[p + 9] & 0x02 != 0 {
            body = replace_ff00(&body);
        }
        pos += hdr_len + fs;
        if fs > 0 {
            frames.push((sub(&data, p, id_len as usize).to_vec(), body));
        }
    }
    frames
}

pub(super) fn parse_id3v2(tag: &[u8]) -> Tags {
    let mut out = Tags::default();
    for (id, body) in id3_frames(tag) {
        if id == b"APIC" || id == b"PIC" {
            if body.len() > 100 {
                out.pic = true;
            }
            continue;
        }
        let key = match &id[..] {
            b"TIT2" | b"TT2" => "title",
            b"TPE1" | b"TP1" => "artist",
            b"TALB" | b"TAL" => "album",
            b"TPE2" | b"TP2" => "albumartist",
            b"TRCK" | b"TRK" => "track",
            _ => continue,
        };
        // Stored values are never broken, so the first usable frame wins.
        if out.has(key) {
            continue;
        }
        let val = id3_text(&body);
        if val.is_empty() || is_broken(&val) {
            continue;
        }
        out.set(key, val);
    }
    out
}

/// Text frame body -> UTF-8; multiple strings are joined with " / ".
pub(super) fn id3_text(body: &[u8]) -> String {
    let Some((&enc, s)) = body.split_first() else { return String::new() };
    let t = match enc {
        1 => conv_utf16(s, Utf16::Bom),
        2 => conv_utf16(s, Utf16::Be),
        // PHP keeps raw bytes here and re-decodes the merged value at the end if it is not
        // valid UTF-8; decoding each part right away is the Rust equivalent.
        3 => s.split(|&c| c == 0).map(|p| match std::str::from_utf8(p) {
            Ok(v) => v.to_string(),
            Err(_) => decode_legacy(p),
        }).collect::<Vec<_>>().join("\0"),
        _ => s.split(|&c| c == 0).map(decode_legacy).collect::<Vec<_>>().join("\0"),
    };
    t.split('\0').map(clean_text).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" / ")
}

pub(super) fn parse_id3v1(b: &[u8]) -> Tags {
    let mut out = Tags::default();
    if b.len() != 128 || !b.starts_with(b"TAG") {
        return out;
    }
    out.title = Some(legacy_text(&b[3..33]));
    out.artist = Some(legacy_text(&b[33..63]));
    out.album = Some(legacy_text(&b[63..93]));
    if b[125] == 0 && b[126] != 0 {
        out.track = Some(b[126] as u32);
    }
    out
}

/// Picture from APIC/PIC frames as (mime, data); front cover (type 3) preferred.
pub(super) fn id3_cover(tag: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut best = None;
    for (id, b) in id3_frames(tag) {
        if id != b"APIC" && id != b"PIC" {
            continue;
        }
        let enc = byte(&b, 0);
        let (mime, ptype, mut p);
        if id == b"PIC" {
            mime = if sub(&b, 1, 3).eq_ignore_ascii_case(b"PNG") { "image/png".to_string() } else { "image/jpeg".to_string() };
            ptype = byte(&b, 4);
            p = 5;
        } else {
            let Some(z) = b.iter().skip(1).position(|&c| c == 0).map(|z| z + 1) else { continue };
            let m = String::from_utf8_lossy(&b[1..z]).to_ascii_lowercase();
            mime = if m.contains('/') { m } else if m == "png" { "image/png".into() } else { "image/jpeg".into() };
            ptype = byte(&b, z + 1);
            p = z + 2;
        }
        // Skip the description: UTF-16 ends with two zero bytes, other encodings with one.
        if enc == 1 || enc == 2 {
            let mut q = p;
            while q + 1 < b.len() {
                if b[q] == 0 && b[q + 1] == 0 {
                    break;
                }
                q += 2;
            }
            p = q + 2;
        } else {
            p = match b.get(p..).and_then(|r| r.iter().position(|&c| c == 0)) {
                Some(z) => p + z + 1,
                None => b.len(),
            };
        }
        let pic = (mime, tail(&b, p).to_vec());
        if ptype == 3 {
            return Some(pic);
        }
        if best.is_none() {
            best = Some(pic);
        }
    }
    best
}
