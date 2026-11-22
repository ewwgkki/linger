//! Embedded lyrics readers. Kept out of `Tags`/`AudioInfo` so scanning never stores lyrics.

use std::path::Path;

use super::{byte, conv_utf16, id3, mp4, ogg, sub, tail, u32be, u32le, Src, Utf16};
use crate::text::decode_legacy;

const MAX_TAG: u64 = 16 << 20;

pub(super) fn read_lyrics(path: &Path) -> Option<String> {
    let mut src = Src::open(path)?;
    let head = src.read_at(0, 10);
    let mut id3len = 0u64;
    let mut found = None;
    if head.len() == 10 && head.starts_with(b"ID3") {
        id3len = id3::tag_len(&head);
        found = id3_lyrics(&src.read_at(0, id3len.min(MAX_TAG) as usize));
    }
    if found.is_none() {
        let magic = src.read_at(id3len, 4);
        if magic == b"fLaC" {
            found = flac_lyrics(&mut src, id3len + 4);
        } else if src.read_at(4, 4) == b"ftyp" {
            found = mp4_lyrics(&mut src);
        } else if magic == b"OggS" {
            found = ogg_lyrics(&mut src, id3len);
        }
    }
    if found.is_none() {
        found = ape_lyrics(&mut src);
    }
    found.and_then(|s| finish(&s))
}

/// Normalises line breaks, drops NULs and outer blank space; `None` when nothing is left.
fn finish(s: &str) -> Option<String> {
    let s = s.replace("\r\n", "\n").replace('\r', "\n").replace('\0', "");
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Raw bytes that should be UTF-8 but may be legacy-encoded (GBK etc.).
fn text_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => decode_legacy(b),
    }
}

/// Decodes `b` per ID3 encoding byte; no terminator handling.
fn id3_decode(enc: u8, b: &[u8]) -> String {
    match enc {
        1 => conv_utf16(b, Utf16::Bom),
        2 => conv_utf16(b, Utf16::Be),
        3 => text_bytes(b),
        _ => decode_legacy(b),
    }
}

/// Index just past the terminator starting at/after `from` (two aligned zeros for UTF-16).
/// Returns (end of text, start of what follows).
fn find_term(b: &[u8], from: usize, enc: u8) -> (usize, usize) {
    if enc == 1 || enc == 2 {
        let mut q = from;
        while q + 1 < b.len() {
            if b[q] == 0 && b[q + 1] == 0 {
                return (q, q + 2);
            }
            q += 2;
        }
        (b.len(), b.len())
    } else {
        match b.get(from..).and_then(|r| r.iter().position(|&c| c == 0)) {
            Some(z) => (from + z, from + z + 1),
            None => (b.len(), b.len()),
        }
    }
}

fn id3_lyrics(tag: &[u8]) -> Option<String> {
    let frames = id3::id3_frames(tag);
    // time-stamped lyrics (ms) win over unsynchronised ones
    for (id, body) in &frames {
        if id == b"SYLT" || id == b"SLT" {
            if let Some(s) = sylt_to_lrc(body) {
                return Some(s);
            }
        }
    }
    for (id, body) in &frames {
        if id == b"USLT" || id == b"ULT" {
            let enc = byte(body, 0);
            let (_, p) = find_term(body, 4, enc);
            let s = id3_decode(enc, tail(body, p));
            if finish(&s).is_some() {
                return Some(s);
            }
        }
    }
    None
}

fn sylt_to_lrc(b: &[u8]) -> Option<String> {
    let enc = byte(b, 0);
    if b.len() < 6 || byte(b, 4) != 2 {
        return None;
    }
    let (_, mut p) = find_term(b, 6, enc);
    let mut out = String::new();
    while p < b.len() {
        let (e, next) = find_term(b, p, enc);
        if next + 4 > b.len() {
            break;
        }
        let text = id3_decode(enc, &b[p..e]);
        let ms = u32be(b, next);
        p = next + 4;
        let text = text.replace("\r\n", "\n");
        let text = text.strip_prefix('\n').unwrap_or(&text).trim_end_matches(['\n', '\r']);
        let cs = ms / 10;
        out.push_str(&format!("[{:02}:{:02}.{:02}]{}\n", cs / 6000, cs / 100 % 60, cs % 100, text));
    }
    (!out.is_empty()).then_some(out)
}

fn is_lyrics_key(k: &[u8]) -> bool {
    k.eq_ignore_ascii_case(b"lyrics") || k.eq_ignore_ascii_case(b"unsyncedlyrics")
}

/// LYRICS / UNSYNCEDLYRICS from a Vorbis comment list starting at `start` in `b`.
fn vorbis_lyrics(b: &[u8], start: usize) -> Option<String> {
    let mut p = start.checked_add(4)?.checked_add(u32le(b, start) as usize)?;
    let cnt = u32le(b, p);
    p = p.checked_add(4)?;
    let mut i = 0;
    while i < cnt && p < b.len() {
        let l = u32le(b, p) as usize;
        let kv = sub(b, p.saturating_add(4), l);
        p = p.saturating_add(4).saturating_add(l);
        i += 1;
        let Some(eq) = kv.iter().position(|&c| c == b'=') else { continue };
        if is_lyrics_key(&kv[..eq]) {
            let s = text_bytes(&kv[eq + 1..]);
            if finish(&s).is_some() {
                return Some(s);
            }
        }
    }
    None
}

fn flac_lyrics(src: &mut Src, mut pos: u64) -> Option<String> {
    for _ in 0..64 {
        let h = src.read_at(pos, 4);
        if h.len() < 4 {
            return None;
        }
        pos += 4;
        let last = h[0] & 0x80 != 0;
        let len = (h[1] as usize) << 16 | (h[2] as usize) << 8 | h[3] as usize;
        if h[0] & 0x7F == 4 {
            return vorbis_lyrics(&src.read_at(pos, len), 0);
        }
        pos += len as u64;
        if last {
            return None;
        }
    }
    None
}

fn ogg_lyrics(src: &mut Src, start: u64) -> Option<String> {
    let (_, packets) = ogg::header_packets(src, start)?;
    let off = if packets[0].starts_with(b"\x01vorbis") {
        7
    } else if packets[0].starts_with(b"OpusHead") {
        8
    } else {
        return None;
    };
    vorbis_lyrics(&packets[1], off)
}

fn mp4_lyrics(src: &mut Src) -> Option<String> {
    let moov = mp4::find_moov(src)?;
    let data = mp4::find(&moov, &[b"udta", b"meta", b"ilst", b"\xA9lyr", b"data"], 0)?;
    (data.len() >= 8).then(|| text_bytes(tail(data, 8)))
}

/// APEv2 tail tag (optionally before an ID3v1 tag): item "Lyrics".
fn ape_lyrics(src: &mut Src) -> Option<String> {
    let size = src.size;
    if size < 160 {
        return None;
    }
    let has_v1 = src.read_at(size - 128, 3) == b"TAG";
    let foot_pos = size - 32 - if has_v1 { 128 } else { 0 };
    let foot = src.read_at(foot_pos, 32);
    if !foot.starts_with(b"APETAGEX") {
        return None;
    }
    let tag_size = u32le(&foot, 12) as u64;
    let count = u32le(&foot, 16);
    if tag_size > MAX_TAG || tag_size > foot_pos + 32 {
        return None;
    }
    let items = src.read_at(foot_pos + 32 - tag_size, tag_size.saturating_sub(32) as usize);
    let mut p = 0usize;
    for _ in 0..count {
        if p + 8 >= items.len() {
            break;
        }
        let vl = u32le(&items, p) as usize;
        let flags = u32le(&items, p + 4);
        let z = items[p + 8..].iter().position(|&c| c == 0)? + p + 8;
        let key = &items[p + 8..z];
        let val = sub(&items, z + 1, vl);
        p = z.saturating_add(1).saturating_add(vl);
        if flags & 0x06 == 0 && key.eq_ignore_ascii_case(b"lyrics") {
            let s = text_bytes(val);
            if finish(&s).is_some() {
                return Some(s);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::lyrics::parse_lrc;
    use crate::tags::read_lyrics;
    use crate::tags::testutil::*;

    fn uslt(text: &[u8]) -> Vec<u8> {
        [&[0u8][..], b"eng", &[0], text].concat()
    }

    fn sylt(items: &[(&str, u32)]) -> Vec<u8> {
        let mut b = vec![0u8];
        b.extend_from_slice(b"eng");
        b.extend_from_slice(&[2, 1, 0]); // ms, lyrics, empty descriptor
        for (t, ms) in items {
            b.extend_from_slice(t.as_bytes());
            b.push(0);
            b.extend_from_slice(&ms.to_be_bytes());
        }
        b
    }

    fn mp3_with(frames: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
        write_temp("a.mp3", &[id3v2_23(frames), mp3_cbr_frames(20)].concat())
    }

    #[test]
    fn reads_id3_uslt() {
        let p = mp3_with(&[("USLT", uslt(b"Line one\r\nLine two"))]);
        assert_eq!(read_lyrics(&p).as_deref(), Some("Line one\nLine two"));
    }

    #[test]
    fn reads_id3_uslt_utf16() {
        let mut t = vec![1u8];
        t.extend_from_slice(b"eng");
        t.extend_from_slice(&[0xFF, 0xFE, 0, 0]); // BOM descriptor, terminator
        t.extend_from_slice(&[0xFF, 0xFE]);
        for u in "A\nB".encode_utf16() {
            t.extend_from_slice(&u.to_le_bytes());
        }
        let p = mp3_with(&[("USLT", t)]);
        assert_eq!(read_lyrics(&p).as_deref(), Some("A\nB"));
    }

    #[test]
    fn reads_id3v22_ult() {
        let body = uslt(b"Old tag");
        let mut frames = b"ULT".to_vec();
        frames.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        frames.extend_from_slice(&body);
        let n = frames.len() as u32;
        let mut tag = b"ID3\x02\x00\x00".to_vec();
        tag.extend_from_slice(&[(n >> 21 & 0x7F) as u8, (n >> 14 & 0x7F) as u8, (n >> 7 & 0x7F) as u8, (n & 0x7F) as u8]);
        tag.extend_from_slice(&frames);
        let p = write_temp("b.mp3", &[tag, mp3_cbr_frames(20)].concat());
        assert_eq!(read_lyrics(&p).as_deref(), Some("Old tag"));
    }

    #[test]
    fn reads_id3_sylt_as_lrc() {
        let p = mp3_with(&[
            ("USLT", uslt(b"plain fallback")),
            ("SYLT", sylt(&[("\nHello", 1500), ("\nWorld", 62340)])),
        ]);
        let text = read_lyrics(&p).expect("lyrics");
        let parsed = parse_lrc(&text);
        let got: Vec<(Option<u32>, &str)> = parsed.lines.iter().map(|l| (l.start, l.text.as_str())).collect();
        assert_eq!(got, vec![(Some(1500), "Hello"), (Some(62340), "World")]);
    }

    #[test]
    fn sylt_in_frames_is_ignored() {
        let mut b = sylt(&[("x", 1)]);
        b[4] = 1; // MPEG frames, not ms
        let p = mp3_with(&[("SYLT", b), ("USLT", uslt(b"plain"))]);
        assert_eq!(read_lyrics(&p).as_deref(), Some("plain"));
    }

    #[test]
    fn uslt_latin1_gbk_is_repaired() {
        let p = mp3_with(&[("USLT", uslt(&gbk("晴天")))]);
        assert_eq!(read_lyrics(&p).as_deref(), Some("晴天"));
    }

    #[test]
    fn reads_flac_lyrics() {
        let p = write_temp("a.flac", &flac_file(&[("TITLE", "t"), ("LYRICS", "[00:01.00]Hi")], None));
        assert_eq!(read_lyrics(&p).as_deref(), Some("[00:01.00]Hi"));
    }

    #[test]
    fn reads_vorbis_unsynced() {
        let cm = [b"\x03vorbis".to_vec(), vorbis_comment_list(&[("UnsyncedLyrics", "Words\nmore")])].concat();
        let ogg = ogg_stream(b"\x01vorbis\0\0\0\0\x01\x44\xac\0\0".to_vec(), cm, 44100);
        let p = write_temp("a.ogg", &ogg);
        assert_eq!(read_lyrics(&p).as_deref(), Some("Words\nmore"));
    }

    fn atom(t: &[u8], body: &[u8]) -> Vec<u8> {
        [&((body.len() + 8) as u32).to_be_bytes()[..], t, body].concat()
    }

    #[test]
    fn reads_mp4_lyr() {
        let data = atom(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], b"M4A words"].concat());
        let ilst = atom(b"ilst", &atom(b"\xA9lyr", &data));
        let meta = atom(b"meta", &[&[0u8; 4][..], &ilst].concat());
        let moov = atom(b"moov", &atom(b"udta", &meta));
        let p = write_temp("a.m4a", &[atom(b"ftyp", b"M4A \0\0\0\0"), moov].concat());
        assert_eq!(read_lyrics(&p).as_deref(), Some("M4A words"));
    }

    #[test]
    fn reads_ape_lyrics_tail() {
        let val = "[00:02.00]Tail";
        let mut items = Vec::new();
        items.extend_from_slice(&(val.len() as u32).to_le_bytes());
        items.extend_from_slice(&0u32.to_le_bytes());
        items.extend_from_slice(b"Lyrics\0");
        items.extend_from_slice(val.as_bytes());
        let mut foot = b"APETAGEX".to_vec();
        foot.extend_from_slice(&2000u32.to_le_bytes());
        foot.extend_from_slice(&(items.len() as u32 + 32).to_le_bytes());
        foot.extend_from_slice(&1u32.to_le_bytes());
        foot.extend_from_slice(&[0u8; 12]);
        let p = write_temp("c.mp3", &[mp3_cbr_frames(20), items, foot].concat());
        assert_eq!(read_lyrics(&p).as_deref(), Some(val));
    }

    #[test]
    fn no_lyrics_is_none() {
        let p = mp3_with(&[("TIT2", latin1_frame(b"x"))]);
        assert_eq!(read_lyrics(&p), None);
        let p = write_temp("n.flac", &flac_file(&[("TITLE", "t")], None));
        assert_eq!(read_lyrics(&p), None);
    }

    #[test]
    fn corrupt_file_is_none() {
        let p = write_temp("x.mp3", b"ID3\x03\x00\x00\x7f\x7f\x7f\x7fUSLT\xff\xff\xff\xff");
        assert_eq!(read_lyrics(&p), None);
        let p = write_temp("y.flac", b"fLaC\x04\xff\xff\xffjunk");
        assert_eq!(read_lyrics(&p), None);
        assert_eq!(read_lyrics(std::path::Path::new("/definitely/missing.mp3")), None);
    }
}
