//! Synthetic audio file builders for tests (reused by scanner / cover tests).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// ID3v2.3 tag containing the given frames (id, body).
pub fn id3v2_23(frames: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, data) in frames {
        body.extend_from_slice(&id.as_bytes()[..4]);
        body.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(data);
    }
    let n = body.len() as u32;
    let mut out = b"ID3\x03\x00\x00".to_vec();
    out.extend_from_slice(&[(n >> 21 & 0x7F) as u8, (n >> 14 & 0x7F) as u8, (n >> 7 & 0x7F) as u8, (n & 0x7F) as u8]);
    out.extend_from_slice(&body);
    out
}

/// Text frame body with encoding byte 0 (Latin-1) followed by raw bytes.
pub fn latin1_frame(bytes: &[u8]) -> Vec<u8> {
    [&[0u8][..], bytes].concat()
}

/// GBK encoding of `s`.
pub fn gbk(s: &str) -> Vec<u8> {
    encoding_rs::GBK.encode(s).0.into_owned()
}

/// APIC frame body: Latin-1, mime "image/jpeg", picture type, empty description, data.
pub fn apic_frame(ptype: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8];
    b.extend_from_slice(b"image/jpeg\0");
    b.push(ptype);
    b.push(0);
    b.extend_from_slice(data);
    b
}

/// `n` MPEG-1 Layer III frames, 128 kbps / 44.1 kHz, 417 bytes each, zero payload.
pub fn mp3_cbr_frames(n: usize) -> Vec<u8> {
    let mut frame = vec![0u8; 417];
    frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
    frame.repeat(n)
}

/// 16-bit stereo PCM WAV of `secs` seconds; `extra_chunks` go before the data chunk.
pub fn wav_pcm(secs: f64, rate: u32, extra_chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let data_len = ((secs * rate as f64).round() as u32) * 4;
    let mut chunks = Vec::new();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 4).to_le_bytes());
    fmt.extend_from_slice(&4u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let mut push = |id: &[u8], body: &[u8]| {
        chunks.extend_from_slice(id);
        chunks.extend_from_slice(&(body.len() as u32).to_le_bytes());
        chunks.extend_from_slice(body);
        if body.len() & 1 == 1 {
            chunks.push(0);
        }
    };
    push(b"fmt ", &fmt);
    for (id, body) in extra_chunks {
        push(&id[..], body);
    }
    push(b"data", &vec![0u8; data_len as usize]);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + chunks.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&chunks);
    out
}

/// FLAC file (44.1 kHz, 10 s) with a Vorbis comment block and an optional picture (type, data).
pub fn flac_file(comments: &[(&str, &str)], picture: Option<(u32, Vec<u8>)>) -> Vec<u8> {
    let mut si = vec![0u8; 34];
    si[0..2].copy_from_slice(&4096u16.to_be_bytes());
    si[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let (sr, ch, bps, total): (u32, u32, u32, u64) = (44100, 2, 16, 441000);
    si[10] = (sr >> 12) as u8;
    si[11] = (sr >> 4) as u8;
    si[12] = (((sr & 0xF) << 4) | ((ch - 1) << 1) | ((bps - 1) >> 4)) as u8;
    si[13] = ((((bps - 1) & 0xF) << 4) as u64 | (total >> 32 & 0xF)) as u8;
    si[14..18].copy_from_slice(&(total as u32).to_be_bytes());

    let mut vc = Vec::new();
    vc.extend_from_slice(&6u32.to_le_bytes());
    vc.extend_from_slice(b"linger");
    vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for (k, v) in comments {
        let kv = format!("{k}={v}");
        vc.extend_from_slice(&(kv.len() as u32).to_le_bytes());
        vc.extend_from_slice(kv.as_bytes());
    }

    let mut blocks: Vec<(u8, Vec<u8>)> = vec![(0, si), (4, vc)];
    if let Some((ptype, data)) = picture {
        blocks.push((6, flac_picture_block(ptype, "image/jpeg", &data)));
    }
    let mut out = b"fLaC".to_vec();
    let last = blocks.len() - 1;
    for (i, (t, body)) in blocks.into_iter().enumerate() {
        out.push(t | if i == last { 0x80 } else { 0 });
        let n = body.len() as u32;
        out.extend_from_slice(&n.to_be_bytes()[1..]);
        out.extend_from_slice(&body);
    }
    out.extend_from_slice(&[0xFF, 0xF8, 0, 0, 0, 0, 0, 0]);
    out
}

/// Body of a FLAC PICTURE metadata block (also used base64-encoded in Ogg comments).
pub fn flac_picture_block(ptype: u32, mime: &str, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&ptype.to_be_bytes());
    b.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    b.extend_from_slice(mime.as_bytes());
    b.extend_from_slice(&0u32.to_be_bytes()); // description
    b.extend_from_slice(&[0u8; 16]); // width, height, depth, colors
    b.extend_from_slice(&(data.len() as u32).to_be_bytes());
    b.extend_from_slice(data);
    b
}

/// One Ogg page carrying whole `packets` (each < 64 KiB in total). CRC is left 0.
pub fn ogg_page(serial: u32, seq: u32, granule: u64, header_type: u8, packets: &[Vec<u8>]) -> Vec<u8> {
    let mut lacing = Vec::new();
    for p in packets {
        let mut n = p.len();
        while n >= 255 {
            lacing.push(255u8);
            n -= 255;
        }
        lacing.push(n as u8);
    }
    let mut out = b"OggS\0".to_vec();
    out.push(header_type);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    for p in packets {
        out.extend_from_slice(p);
    }
    out
}

/// Ogg stream: identification header page, comment header page, one audio page ending at `last_granule`.
pub fn ogg_stream(id_header: Vec<u8>, comment_header: Vec<u8>, last_granule: u64) -> Vec<u8> {
    let serial = 0x1234_5678;
    [
        ogg_page(serial, 0, 0, 0x02, &[id_header]),
        ogg_page(serial, 1, 0, 0x00, &[comment_header]),
        ogg_page(serial, 2, last_granule, 0x04, &[vec![0u8; 300]]),
    ]
    .concat()
}

/// Vorbis comment list body (vendor + comments), without any codec prefix.
pub fn vorbis_comment_list(comments: &[(&str, &str)]) -> Vec<u8> {
    let mut vc = Vec::new();
    vc.extend_from_slice(&6u32.to_le_bytes());
    vc.extend_from_slice(b"linger");
    vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for (k, v) in comments {
        let kv = format!("{k}={v}");
        vc.extend_from_slice(&(kv.len() as u32).to_le_bytes());
        vc.extend_from_slice(kv.as_bytes());
    }
    vc
}

/// Writes `bytes` to a fresh temp directory under `name` and returns the path.
pub fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "linger-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write temp file");
    p
}
