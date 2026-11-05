//! Ogg Vorbis / Opus (new, not in the PHP original): comments, cover and duration.

use super::flac::{picture_block, vorbis_comments};
use super::{u16le, u32le, AudioInfo, Src};

/// Header packets larger than this are treated as corrupt.
const MAX_HEADER: usize = 64 << 20;

struct Page {
    header_type: u8,
    serial: u32,
    lacing: Vec<u8>,
    body_pos: u64,
}

fn parse_page_header(h: &[u8]) -> Option<(u8, u64, u32, usize)> {
    if h.len() < 27 || &h[..4] != b"OggS" {
        return None;
    }
    let granule = u64::from_le_bytes(h[6..14].try_into().ok()?);
    Some((h[5], granule, u32le(h, 14), h[26] as usize))
}

fn read_page(src: &mut Src, pos: u64) -> Option<Page> {
    let h = src.read_at(pos, 27);
    let (header_type, _, serial, nseg) = parse_page_header(&h)?;
    let lacing = src.read_at(pos + 27, nseg);
    if lacing.len() < nseg {
        return None;
    }
    Some(Page { header_type, serial, lacing, body_pos: pos + 27 + nseg as u64 })
}

/// First two packets (identification + comment header) of the first logical stream.
fn header_packets(src: &mut Src, start: u64) -> Option<(u32, Vec<Vec<u8>>)> {
    let mut pos = start;
    let mut serial = None;
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for _ in 0..4096 {
        let page = read_page(src, pos)?;
        let body_len: u64 = page.lacing.iter().map(|&l| l as u64).sum();
        pos = page.body_pos + body_len;
        if *serial.get_or_insert(page.serial) != page.serial {
            continue; // other multiplexed stream
        }
        let body = src.read_at(page.body_pos, body_len as usize);
        let mut off = 0usize;
        for &l in &page.lacing {
            let seg = body.get(off..off + l as usize)?;
            off += l as usize;
            cur.extend_from_slice(seg);
            if cur.len() > MAX_HEADER {
                return None;
            }
            if l < 255 {
                packets.push(std::mem::take(&mut cur));
                if packets.len() == 2 {
                    return Some((page.serial, packets));
                }
            }
        }
        if page.header_type & 0x04 != 0 {
            break;
        }
    }
    None
}

enum Codec {
    Vorbis { rate: u32 },
    Opus { pre_skip: u16 },
}

fn identify(id: &[u8]) -> Option<Codec> {
    if id.starts_with(b"\x01vorbis") {
        Some(Codec::Vorbis { rate: u32le(id, 12) })
    } else if id.starts_with(b"OpusHead") {
        Some(Codec::Opus { pre_skip: u16le(id, 10) })
    } else {
        None
    }
}

/// Granule position of the last page of `serial` (searches the last 256 KiB).
fn last_granule(src: &mut Src, serial: u32) -> Option<u64> {
    let from = src.size.saturating_sub(256 << 10);
    let buf = src.read_at(from, 256 << 10);
    let mut i = buf.len().checked_sub(27)?;
    loop {
        if &buf[i..i + 4] == b"OggS" {
            if let Some((_, g, s, _)) = parse_page_header(&buf[i..]) {
                if s == serial && g != u64::MAX {
                    return Some(g);
                }
            }
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

pub(super) fn read_ogg(src: &mut Src, start: u64, info: &mut AudioInfo) {
    let Some((serial, packets)) = header_packets(src, start) else { return };
    let Some(codec) = identify(&packets[0]) else { return };
    let comment_off = match codec {
        Codec::Vorbis { .. } => {
            info.codec = "ogg".into();
            7
        }
        Codec::Opus { .. } => {
            info.codec = "opus".into();
            8
        }
    };
    let cm = &packets[1];
    let (tags, pics) = vorbis_comments(cm, comment_off, cm.len());
    info.merge(tags);
    if pics.iter().any(|b64| picture_block(&b64_decode(b64)).2.len() > 100) {
        info.pic = true;
    }
    if let Some(g) = last_granule(src, serial) {
        info.duration = match codec {
            Codec::Vorbis { rate } if rate > 0 => g as f64 / rate as f64,
            Codec::Opus { pre_skip } => g.saturating_sub(pre_skip as u64) as f64 / 48000.0,
            _ => 0.0,
        };
    }
}

/// METADATA_BLOCK_PICTURE cover as (mime, data), front cover preferred.
pub(super) fn ogg_cover(src: &mut Src, start: u64) -> Option<(String, Vec<u8>)> {
    let (_, packets) = header_packets(src, start)?;
    let off = match identify(&packets[0])? {
        Codec::Vorbis { .. } => 7,
        Codec::Opus { .. } => 8,
    };
    let (_, pics) = vorbis_comments(&packets[1], off, packets[1].len());
    let mut best = None;
    for b64 in pics {
        let (ptype, mime, data) = picture_block(&b64_decode(&b64));
        if ptype == 3 {
            return Some((mime, data));
        }
        if best.is_none() {
            best = Some((mime, data));
        }
    }
    best
}

/// Lenient standard base64 decoder (skips whitespace and padding, stops at invalid input).
fn b64_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => break,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}
