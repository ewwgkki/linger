//! Monkey's Audio (port of read_ape) and APEv2 tail tags (APE part of read_tail_tags).

use super::{legacy_text, sub, u16le, u32le, AudioInfo, Src, Tags};

pub(super) fn read_ape(src: &mut Src, start: u64, info: &mut AudioInfo) {
    info.codec = "ape".into();
    let b = src.read_at(start, 128);
    let ver = u16le(&b, 4);
    let (bpf, fin, frames, sr);
    if ver >= 3980 {
        let desc_len = u32le(&b, 8) as usize;
        let mut h = sub(&b, desc_len, 24).to_vec();
        if h.len() < 24 {
            h = src.read_at(start + desc_len as u64, 24);
        }
        bpf = u32le(&h, 4) as f64;
        fin = u32le(&h, 8) as f64;
        frames = u32le(&h, 12) as f64;
        sr = u32le(&h, 20) as f64;
    } else {
        let comp = u16le(&b, 6);
        sr = u32le(&b, 12) as f64;
        frames = u32le(&b, 24) as f64;
        fin = u32le(&b, 28) as f64;
        bpf = if ver >= 3950 {
            73728.0 * 4.0
        } else if ver >= 3900 || (ver >= 3800 && comp == 4000) {
            73728.0
        } else {
            9216.0
        };
    }
    if sr != 0.0 && frames != 0.0 {
        info.duration = ((frames - 1.0) * bpf + fin) / sr;
    }
}

/// Largest APEv2 tag (incl. footer) we are willing to read.
const MAX_APE_TAG: u64 = 16 << 20;

/// APEv2 items before the footer at `foot_pos`.
pub(super) fn parse_apev2(src: &mut Src, foot_pos: u64, foot: &[u8]) -> Tags {
    let tag_size = u32le(foot, 12) as u64;
    let count = u32le(foot, 16);
    // A corrupt footer can claim up to 4 GiB; real tags are far below 16 MiB and always fit
    // before the footer. Skip such tags instead of reading (and allocating) most of the file.
    if tag_size > MAX_APE_TAG || tag_size > foot_pos + 32 {
        return Tags::default();
    }
    let items = src.read_at(foot_pos + 32 - tag_size, tag_size.saturating_sub(32) as usize);
    let mut tags = Tags::default();
    let mut p = 0usize;
    let mut i = 0;
    while i < count && p + 8 < items.len() {
        i += 1;
        let vl = u32le(&items, p) as usize;
        let Some(z) = items[p + 8..].iter().position(|&c| c == 0).map(|z| z + p + 8) else { break };
        let key = items[p + 8..z].to_ascii_lowercase();
        let val = legacy_text(sub(&items, z + 1, vl));
        p = z.saturating_add(1).saturating_add(vl);
        let k = match &key[..] {
            b"title" => "title",
            b"artist" => "artist",
            b"album" => "album",
            b"album artist" => "albumartist",
            b"track" => "track",
            _ => continue,
        };
        tags.set(k, val);
    }
    tags
}
