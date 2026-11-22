//! Local lyric sources: sibling `.lrc` files and their text decoding.

use std::path::Path;

const MAX_LRC: u64 = 2 << 20;

/// BOM (UTF-8/16LE/16BE) -> valid UTF-8 -> GBK.
pub fn decode_lrc(bytes: &[u8]) -> String {
    use encoding_rs::{GBK, UTF_16BE, UTF_16LE};
    if let Some(b) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(b).into_owned();
    }
    if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return UTF_16LE.decode_without_bom_handling(b).0.into_owned();
    }
    if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return UTF_16BE.decode_without_bom_handling(b).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => GBK.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// Text of the `.lrc` next to `audio` with the same stem (extension case-insensitive, <= 2 MiB).
pub fn sibling_lrc(audio: &Path) -> Option<String> {
    let dir = audio.parent()?;
    let stem = audio.file_stem()?.to_string_lossy().to_lowercase();
    let mut best: Option<std::path::PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let is_lrc = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lrc"));
        if !is_lrc || p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).as_deref() != Some(&stem) {
            continue;
        }
        if !e.metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_LRC) {
            continue;
        }
        // prefer the exact-case stem when several differ only by case
        if p.file_stem() == audio.file_stem() || best.is_none() {
            best = Some(p);
        }
    }
    let text = decode_lrc(&std::fs::read(best?).ok()?);
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::testutil::write_temp;

    #[test]
    fn lrc_file_gbk_and_utf16() {
        let s = "[00:01.00]晴天\n[00:02.00]你好";
        let gbk = encoding_rs::GBK.encode(s).0.into_owned();
        let mut u16le = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            u16le.extend_from_slice(&u.to_le_bytes());
        }
        let mut u16be = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            u16be.extend_from_slice(&u.to_be_bytes());
        }
        let u8bom = [&[0xEF, 0xBB, 0xBF][..], s.as_bytes()].concat();
        for b in [gbk, u16le, u16be, u8bom, s.as_bytes().to_vec()] {
            assert_eq!(decode_lrc(&b), s);
        }
    }

    #[test]
    fn sibling_is_case_insensitive() {
        let a = write_temp("歌.mp3", b"x");
        std::fs::write(a.with_file_name("歌.LRC"), "[00:01.00]hi").unwrap();
        assert_eq!(sibling_lrc(&a).as_deref(), Some("[00:01.00]hi"));
        let b = write_temp("歌.mp3", b"x");
        std::fs::write(b.with_file_name("别的.lrc"), "no").unwrap();
        assert_eq!(sibling_lrc(&b), None);
    }

    #[test]
    fn sibling_over_2mib_is_skipped() {
        let a = write_temp("big.mp3", b"x");
        std::fs::write(a.with_extension("lrc"), vec![b'a'; (2 << 20) + 1]).unwrap();
        assert_eq!(sibling_lrc(&a), None);
    }
}
