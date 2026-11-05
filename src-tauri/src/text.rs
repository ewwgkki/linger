//! Legacy text decoding and tag cleaning (port of lib/tags.php and api.php helpers).

use encoding_rs::{BIG5, GBK, UTF_16BE, UTF_16LE};
use regex::Regex;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("valid regex"))
}

/// Port of `is_broken`.
pub fn is_broken(s: &str) -> bool {
    static A: OnceLock<Regex> = OnceLock::new();
    static B: OnceLock<Regex> = OnceLock::new();
    static C: OnceLock<Regex> = OnceLock::new();
    re(&A, r"\?{2,}|^\?+$").is_match(s)
        // PHP (no /u) \s is ASCII whitespace only
        || re(&B, r"^[?\s()!\-]+$").is_match(s) && s.chars().all(|c| c.is_ascii())
        || re(&C, "[\u{0}-\u{8}\u{E}-\u{1F}]|\u{FFFD}|ÿþ|þÿ").is_match(s)
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Port of `decode_legacy`.
pub fn decode_legacy(bytes: &[u8]) -> String {
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == 0 || bytes[end - 1] == b' ') {
        end -= 1;
    }
    let b = &bytes[..end];
    if b.is_empty() || b.iter().all(|&c| c < 0x80) {
        return latin1(b);
    }
    if b.starts_with(&[0xFF, 0xFE]) {
        return UTF_16LE.decode_without_bom_handling(&b[2..]).0.into_owned();
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        return UTF_16BE.decode_without_bom_handling(&b[2..]).0.into_owned();
    }
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    let mut best: Option<(&'static encoding_rs::Encoding, f64)> = None;
    let cands: [(&'static encoding_rs::Encoding, u8, u8, u8, u8); 2] = [
        (GBK, 0xB0, 0xD7, 0xA1, 0xA9),
        (BIG5, 0xA4, 0xC6, 0xA1, 0xA3),
    ];
    for (enc, lo, hi, sym_lo, sym_hi) in cands {
        let (mut n, mut common) = (0u32, 0u32);
        let mut ok = true;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            i += 1;
            if c < 0x80 {
                continue;
            }
            if i >= b.len() {
                ok = false;
                break;
            }
            let t = b[i];
            i += 1;
            if t < 0x40 {
                ok = false;
                break;
            }
            if c >= sym_lo && c <= sym_hi {
                continue;
            }
            n += 1;
            if c >= lo && c <= hi {
                common += 1;
            }
        }
        if !ok {
            continue;
        }
        let ratio = if n > 0 { common as f64 / n as f64 } else { 1.0 };
        if ratio >= 0.6 && best.map_or(true, |(_, r)| ratio > r) {
            best = Some((enc, ratio));
        }
    }
    match best {
        Some((enc, _)) => enc.decode_without_bom_handling(b).0.into_owned(),
        None => latin1(b),
    }
}

/// Port of `clean_text`.
pub fn clean_text(s: &str) -> String {
    static A: OnceLock<Regex> = OnceLock::new();
    let s = s.replace('\0', "");
    // PHP trim() strips " \t\n\r\0\x0B"
    let s = s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'));
    re(&A, r"\s+").replace_all(s, " ").into_owned()
}

/// Port of `clean_tag` (api.php).
pub fn clean_tag(s: &str) -> String {
    static R1: OnceLock<Regex> = OnceLock::new();
    static R2: OnceLock<Regex> = OnceLock::new();
    static R3: OnceLock<Regex> = OnceLock::new();
    static R4: OnceLock<Regex> = OnceLock::new();
    static R5: OnceLock<Regex> = OnceLock::new();
    static R6: OnceLock<Regex> = OnceLock::new();
    static R7: OnceLock<Regex> = OnceLock::new();
    let v = re(&R1, r"(?i)\[?\s*(https?://|www\.|wap\.|bbs\.)\S+\s*\]?").replace_all(s, "");
    let v = re(
        &R2,
        r"(?i)\s*(搜啦EP|全球华语Demo|快听音乐网|蓝星音乐社区|Fad音[乐樂]论坛|视听天空|想你听歌网|站长\s*[:：]|qq\s*[:：]?\s*\d{5,}|联系\s*qq).*$",
    )
    .replace_all(&v, "");
    let v = re(&R3, r"(?i)\[?[\w-]+\.(com|cn|net|org|cc)\]?").replace_all(&v, "");
    let v = v.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'));
    let v = re(&R4, r"^《(.+)》$").replace(v, "$1");
    let v = re(&R5, r"^[\s\-_|·]+|[\s\-_|·]+$").replace_all(&v, "");
    let v = re(&R6, r"\s{2,}").replace_all(&v, " ").into_owned();
    if v.is_empty()
        || is_broken(&v)
        || re(
            &R7,
            r"(?i)^(track\s*\d+|曲目\s*\d+|未知.*|unknown.*|artist|title|<undefined>|单曲|single)$|论坛$|收藏$",
        )
        .is_match(&v)
    {
        return String::new();
    }
    v
}

/// Port of `is_unknown`.
pub fn is_unknown(s: &str) -> bool {
    static A: OnceLock<Regex> = OnceLock::new();
    re(&A, r"(?i)^(未知|unknown)").is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test] fn gbk_bytes_decode() { assert_eq!(decode_legacy(&hex("d6dcbddcc2d7")), "周杰伦"); }
    #[test] fn latin1_fallback() { assert_eq!(decode_legacy(&hex("436166e9")), "Café"); }
    #[test] fn ascii_and_utf8_passthrough() { assert_eq!(decode_legacy(b"Hello"), "Hello"); assert_eq!(decode_legacy("晴天".as_bytes()), "晴天"); }
    #[test] fn utf16_bom() { assert_eq!(decode_legacy(&[0xFF,0xFE,0x74,0x66,0x29,0x59]), "晴天"); }
    #[test] fn broken_detection() {
        assert!(is_broken("??")); assert!(is_broken("? ? ?")); assert!(is_broken("ÿþabc"));
        assert!(!is_broken("Hello")); assert!(!is_broken("晴天"));
    }
    #[test] fn clean_tag_cases() {
        assert_eq!(clean_tag("周杰伦 www.abc.com"), "周杰伦");
        assert_eq!(clean_tag("Track 01"), "");
        assert_eq!(clean_tag("《范特西》"), "范特西");
        assert_eq!(clean_tag("未知艺术家"), "");
        assert_eq!(clean_tag("快听音乐网 周杰伦"), "");
        assert_eq!(clean_tag("  - 晴天 _ "), "晴天");
    }
    #[test] fn misc() {
        assert_eq!(clean_text(" a \0 b\t\nc "), "a b c");
        assert!(is_unknown("Unknown Artist")); assert!(!is_unknown("周杰伦"));
    }
}
