//! MPEG audio (port of read_mpeg, mpeg_count_frames, mpeg_frame).

use super::{php_round, sub, u32be, AudioInfo, Src};

#[derive(Clone, Copy)]
struct Frame {
    br: u32,
    sr: u32,
    spf: u32,
    len: usize,
    xoff: usize,
}

const BR_1_1: [u32; 15] = [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448];
const BR_1_2: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384];
const BR_1_3: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const BR_2_1: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256];
const BR_2_2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

fn mpeg_frame(buf: &[u8], i: usize) -> Option<Frame> {
    let h = buf.get(i..i.checked_add(4)?)?;
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let ver = (h[1] >> 3) & 3;
    let layer = (h[1] >> 1) & 3;
    let bri = (h[2] >> 4) as usize;
    let sri = ((h[2] >> 2) & 3) as usize;
    let pad = ((h[2] >> 1) & 1) as u32;
    if ver == 1 || layer == 0 || bri == 0 || bri == 15 || sri == 3 {
        return None;
    }
    let lay = 4 - layer;
    let table = match (ver == 3, lay) {
        (true, 1) => &BR_1_1,
        (true, 2) => &BR_1_2,
        (true, _) => &BR_1_3,
        (false, 1) => &BR_2_1,
        (false, _) => &BR_2_2,
    };
    let br = table[bri];
    let sr = match ver {
        3 => [44100, 48000, 32000][sri],
        2 => [22050, 24000, 16000][sri],
        _ => [11025, 12000, 8000][sri],
    };
    let spf = if lay == 1 { 384 } else if lay == 3 && ver != 3 { 576 } else { 1152 };
    let len = if lay == 1 { (12 * br * 1000 / sr + pad) * 4 } else { spf / 8 * br * 1000 / sr + pad } as usize;
    if len < 21 {
        return None;
    }
    let mono = (h[3] >> 6) & 3 == 3;
    let xoff = if ver == 3 { if mono { 21 } else { 36 } } else if mono { 13 } else { 21 };
    Some(Frame { br, sr, spf, len, xoff })
}

pub(super) fn read_mpeg(src: &mut Src, start: u64, info: &mut AudioInfo) {
    let size = src.size;
    let buf = src.read_at(start, 262144);
    let n = buf.len();
    let mut i = 0;
    while i + 4 <= n {
        if buf[i] != 0xFF || buf[i + 1] & 0xE0 != 0xE0 {
            i += 1;
            continue;
        }
        let Some(f) = mpeg_frame(&buf, i) else {
            i += 1;
            continue;
        };
        // 4 consecutive frames with the same sample rate mark the real start of audio.
        let (mut j, mut chain) = (i + f.len, 1);
        while chain < 4 && j + 4 <= n {
            match mpeg_frame(&buf, j) {
                Some(g) if g.sr == f.sr => {
                    j += g.len;
                    chain += 1;
                }
                _ => break,
            }
        }
        if chain < 4 && j + 4 <= n {
            i += 1;
            continue;
        }
        let audio_bytes = size as f64 - start as f64 - i as f64;
        let (mut frames, mut bytes) = (0u64, 0u64);
        let tag = sub(&buf, i + f.xoff, 4);
        if tag == b"Xing" || tag == b"Info" {
            let fl = u32be(&buf, i + f.xoff + 4);
            let mut o = i + f.xoff + 8;
            if fl & 1 != 0 {
                frames = u32be(&buf, o) as u64;
                o += 4;
            }
            if fl & 2 != 0 {
                bytes = u32be(&buf, o) as u64;
            }
        } else if sub(&buf, i + 36, 4) == b"VBRI" {
            bytes = u32be(&buf, i + 46) as u64;
            frames = u32be(&buf, i + 50) as u64;
        }
        if frames == 0 {
            // No VBR header: constant bitrate -> CBR maths, otherwise count every frame.
            let (mut vbr, mut p, mut k) = (false, i, 0);
            while k < 200 && p + 4 <= n {
                let Some(x) = mpeg_frame(&buf, p) else { break };
                if x.br != f.br {
                    vbr = true;
                    break;
                }
                p += x.len;
                k += 1;
            }
            if vbr {
                (frames, bytes) = count_frames(src, start + i as u64);
            }
        }
        if frames > 0 {
            info.duration = frames as f64 * f.spf as f64 / f.sr as f64;
            let b = if bytes > 0 { bytes as f64 } else { audio_bytes };
            info.bitrate = php_round(b * 8.0 / info.duration / 1000.0);
        } else {
            info.bitrate = f.br;
            info.duration = audio_bytes * 8.0 / (f.br as f64 * 1000.0);
        }
        info.codec = "mp3".into();
        return;
    }
}

fn count_frames(src: &mut Src, pos: u64) -> (u64, u64) {
    let (mut frames, mut bytes) = (0u64, 0u64);
    let mut chunk: Vec<u8> = Vec::new();
    let (mut base, mut off) = (pos, 0usize);
    loop {
        if off + 4 > chunk.len() {
            base += off as u64;
            off = 0;
            chunk = src.read_at(base, 1 << 20);
            if chunk.len() < 4 {
                break;
            }
        }
        let Some(x) = mpeg_frame(&chunk, off) else { break };
        frames += 1;
        bytes += x.len as u64;
        off += x.len;
    }
    (frames, bytes)
}
