//! WAV / RIFF (port of read_wav, wav_cover).

use super::{id3, legacy_text, php_round, sub, u32le, AudioInfo, Src, Tags, Throw};

/// Chunks as (id, declared length, offset of body); port of the chunk walk loop.
fn chunks(src: &mut Src) -> Vec<([u8; 4], u64, u64)> {
    let riff = src.read_at(0, 12);
    let total = u32le(&riff, 4) as u64 + 8;
    let mut out = Vec::new();
    let mut pos = 12u64;
    for _ in 0..64 {
        if pos + 8 > total {
            break;
        }
        let h = src.read_at(pos, 8);
        if h.len() < 8 {
            break;
        }
        let len = u32le(&h, 4) as u64;
        out.push(([h[0], h[1], h[2], h[3]], len, pos + 8));
        pos += 8 + len + (len & 1);
    }
    out
}

/// `Err` means the PHP original would have thrown (fread with length 0).
pub(super) fn read_wav(src: &mut Src, info: &mut AudioInfo) -> Result<(), Throw> {
    info.codec = "pcm".into();
    if sub(&src.read_at(0, 12), 8, 4) != b"WAVE" {
        return Ok(());
    }
    let (mut byte_rate, mut data_len) = (0u32, 0u64);
    for (id, len, body) in chunks(src) {
        match &id {
            b"fmt " => {
                if len == 0 {
                    return Err(Throw);
                }
                byte_rate = u32le(&src.read_at(body, len.min(64) as usize), 8);
            }
            b"data" => data_len = len,
            b"LIST" if len < 1 << 20 => {
                if len == 0 {
                    return Err(Throw);
                }
                let b = src.read_at(body, len as usize);
                if b.starts_with(b"INFO") {
                    let mut p = 4usize;
                    let mut tags = Tags::default();
                    while p + 8 <= b.len() {
                        let k = &b[p..p + 4];
                        let l = u32le(&b, p + 4) as usize;
                        let v = legacy_text(sub(&b, p + 8, l));
                        let m = match k {
                            b"INAM" => "title",
                            b"IART" => "artist",
                            b"IPRD" => "album",
                            b"ITRK" => "track",
                            _ => "",
                        };
                        tags.set(m, v); // later items overwrite earlier ones, as in PHP
                        p = p.saturating_add(8).saturating_add(l).saturating_add(l & 1);
                    }
                    info.merge(tags);
                }
            }
            _ if id.eq_ignore_ascii_case(b"id3 ") && len < 16 << 20 => {
                if len == 0 {
                    return Err(Throw);
                }
                info.merge(id3::parse_id3v2(&src.read_at(body, len as usize)));
            }
            _ => {}
        }
    }
    if byte_rate != 0 {
        info.duration = data_len as f64 / byte_rate as f64;
        info.bitrate = php_round(byte_rate as f64 * 8.0 / 1000.0);
    }
    Ok(())
}

/// Port of wav_cover: picture from an embedded `id3 ` chunk.
pub(super) fn wav_cover(src: &mut Src) -> Option<(String, Vec<u8>)> {
    for (id, len, body) in chunks(src) {
        if id.eq_ignore_ascii_case(b"id3 ") && len < 32 << 20 {
            return id3::id3_cover(&src.read_at(body, len as usize));
        }
    }
    None
}
