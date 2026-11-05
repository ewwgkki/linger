//! FLAC (port of read_flac, flac_cover) and the shared Vorbis-comment / picture-block parsers.

use super::{byte, legacy_text, sub, u32be, u32le, AudioInfo, Src, Tags, Throw};

/// Walks metadata blocks starting at `pos` (just after "fLaC").
/// `Err` means the PHP original would have thrown (fread with length 0).
pub(super) fn read_flac(src: &mut Src, mut pos: u64, info: &mut AudioInfo) -> Result<(), Throw> {
    info.codec = "flac".into();
    for _ in 0..64 {
        let h = src.read_at(pos, 4);
        if h.len() < 4 {
            return Ok(());
        }
        pos += 4;
        let last = h[0] & 0x80 != 0;
        let typ = h[0] & 0x7F;
        let len = (h[1] as usize) << 16 | (h[2] as usize) << 8 | h[3] as usize;
        if typ == 0 || typ == 4 {
            if len == 0 {
                return Err(Throw);
            }
            let b = src.read_at(pos, len);
            if typ == 0 {
                let sr = (byte(&b, 10) as u32) << 12 | (byte(&b, 11) as u32) << 4 | (byte(&b, 12) as u32) >> 4;
                let total = (byte(&b, 13) & 0x0F) as f64 * 4294967296.0 + u32be(&b, 14) as f64;
                if sr != 0 {
                    info.duration = total / sr as f64;
                }
            } else {
                let (tags, _) = vorbis_comments(&b, 0, len);
                info.merge(tags);
            }
        } else if typ == 6 && len > 100 {
            info.pic = true;
        }
        pos += len as u64;
        if last {
            return Ok(());
        }
    }
    Ok(())
}

/// Port of the FLAC comment loop: vendor, count, "KEY=value" items starting at `start`
/// within `b`, where `limit` is the declared block length.
/// Returns the mapped tags (first value of each key wins) plus every raw `METADATA_BLOCK_PICTURE` value.
pub(super) fn vorbis_comments(b: &[u8], start: usize, limit: usize) -> (Tags, Vec<Vec<u8>>) {
    let mut tags = Tags::default();
    let mut pics = Vec::new();
    let mut p = start + 4 + u32le(b, start) as usize;
    let cnt = u32le(b, p);
    p += 4;
    let mut i = 0;
    while i < cnt && p < limit {
        let l = u32le(b, p) as usize;
        let kv = sub(b, p + 4, l);
        p = p.saturating_add(4).saturating_add(l);
        i += 1;
        let Some(eq) = kv.iter().position(|&c| c == b'=') else { continue };
        let k = kv[..eq].to_ascii_lowercase();
        let raw = &kv[eq + 1..];
        let m = match &k[..] {
            b"title" => "title",
            b"artist" => "artist",
            b"album" => "album",
            b"albumartist" => "albumartist",
            b"tracknumber" => "track",
            b"metadata_block_picture" => {
                pics.push(raw.to_vec());
                continue;
            }
            _ => continue,
        };
        if !tags.has(m) {
            tags.set(m, legacy_text(raw));
        }
    }
    (tags, pics)
}

/// FLAC PICTURE block body -> (picture type, mime, data). Port of the parsing in flac_cover.
pub(super) fn picture_block(b: &[u8]) -> (u32, String, Vec<u8>) {
    let ptype = u32be(b, 0);
    let ml = u32be(b, 4) as usize;
    let mime = String::from_utf8_lossy(sub(b, 8, ml)).into_owned();
    let mut p = 8usize.saturating_add(ml);
    let dl = u32be(b, p) as usize;
    p = p.saturating_add(4).saturating_add(dl).saturating_add(16);
    let data = sub(b, p.saturating_add(4), u32be(b, p) as usize).to_vec();
    (ptype, mime, data)
}

/// Port of flac_cover: (mime, data), front cover preferred.
pub(super) fn flac_cover(src: &mut Src, mut pos: u64) -> Result<Option<(String, Vec<u8>)>, Throw> {
    let mut best = None;
    for _ in 0..64 {
        let h = src.read_at(pos, 4);
        if h.len() < 4 {
            break;
        }
        pos += 4;
        let last = h[0] & 0x80 != 0;
        let typ = h[0] & 0x7F;
        let len = (h[1] as usize) << 16 | (h[2] as usize) << 8 | h[3] as usize;
        if typ == 6 {
            if len == 0 {
                return Err(Throw);
            }
            let (ptype, mime, data) = picture_block(&src.read_at(pos, len));
            if ptype == 3 {
                return Ok(Some((mime, data)));
            }
            if best.is_none() {
                best = Some((mime, data));
            }
        }
        pos += len as u64;
        if last {
            break;
        }
    }
    Ok(best)
}
