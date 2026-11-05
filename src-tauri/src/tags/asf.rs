//! WMA / ASF (port of read_asf).

use super::{conv_utf16, sub, u16le, u32le, u64le, AudioInfo, Src, Tags, Utf16};
use crate::text::clean_text;

/// GUID string -> on-disk byte order (port of guid_bytes).
fn guid_bytes(g: &str) -> Vec<u8> {
    let h: Vec<u8> = g.bytes().filter(|&c| c != b'-').collect();
    let b: Vec<u8> = h.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 16).unwrap_or(0)).collect();
    let mut out = Vec::with_capacity(16);
    out.extend(b[0..4].iter().rev());
    out.extend(b[4..6].iter().rev());
    out.extend(b[6..8].iter().rev());
    out.extend_from_slice(&b[8..]);
    out
}

fn utf16le(b: &[u8]) -> String {
    conv_utf16(b, Utf16::Le)
}

pub(super) fn read_asf(src: &mut Src, info: &mut AudioInfo) {
    info.codec = "wma".into();
    let hdr = src.read_at(0, 30);
    let hsize = u64le(&hdr, 16);
    let count = u32le(&hdr, 24);
    let data = src.read_at(30, hsize.saturating_sub(30).min(4 << 20) as usize);
    let file = guid_bytes("8CABDCA1-A947-11CF-8EE4-00C00C205365");
    let desc = guid_bytes("75B22633-668E-11CF-A6D9-00AA0062CE6C");
    let ext = guid_bytes("D2D0A440-E307-11D2-97F0-00A0C95EA850");
    let mut p = 0usize;
    let mut tags = Tags::default();
    let mut i = 0;
    while i < count && p + 24 <= data.len() {
        i += 1;
        let g = &data[p..p + 16];
        let len = u64le(&data, p + 16);
        if len < 24 {
            break;
        }
        let b = sub(&data, p + 24, (len - 24).min(usize::MAX as u64) as usize);
        if g == &file[..] {
            let play = u64le(b, 40) as f64;
            let preroll = u64le(b, 56) as f64;
            info.duration = (play / 1e7 - preroll / 1000.0).max(0.0);
        } else if g == &desc[..] {
            let (l0, l1) = (u16le(b, 0) as usize, u16le(b, 2) as usize);
            tags.title = Some(clean_text(&utf16le(sub(b, 10, l0))));
            tags.artist = Some(clean_text(&utf16le(sub(b, 10 + l0, l1))));
        } else if g == &ext[..] {
            let n = u16le(b, 0);
            let mut q = 2usize;
            let mut j = 0;
            while j < n && q + 6 <= b.len() {
                j += 1;
                let nl = u16le(b, q) as usize;
                let name = utf16le(sub(b, q + 2, nl));
                q += 2 + nl;
                let vt = u16le(b, q);
                let vl = u16le(b, q + 2) as usize;
                let raw = sub(b, q + 4, vl);
                q += 4 + vl;
                let name = name.trim_end_matches('\0');
                let val = match vt {
                    0 => clean_text(&utf16le(raw)),
                    3 if raw.len() >= 4 => u32le(raw, 0).to_string(),
                    _ => String::new(),
                };
                match name {
                    "WM/AlbumTitle" => tags.album = Some(val),
                    "WM/AlbumArtist" => tags.albumartist = Some(val),
                    "WM/TrackNumber" => tags.set("track", val),
                    _ => {}
                }
            }
        }
        p = match usize::try_from(len).ok().and_then(|l| p.checked_add(l)) {
            Some(np) => np,
            None => break,
        };
    }
    info.merge(tags);
}
