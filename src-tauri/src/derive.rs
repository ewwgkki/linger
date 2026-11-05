//! Song title/artist/album derivation (port of `derive()` in api.php).

use crate::text::{clean_tag, is_unknown};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct RawTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub albumartist: String,
    pub track: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Derived {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track: u32,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("valid regex"))
}

/// PHP `trim()` default character set.
fn php_trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0B'))
}

/// PHP truthiness of a string: "" and "0" are false.
fn truthy(s: &str) -> bool {
    !s.is_empty() && s != "0"
}

fn or(a: String, b: String) -> String {
    if truthy(&a) { a } else { b }
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `^(.+?)\s+-\s+(.+)$` split into trimmed (left, right).
fn split_dash(s: &str) -> Option<(String, String)> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^(.+?)\s+-\s+(.+)$")
        .captures(s)
        .map(|m| (php_trim(&m[1]).to_string(), php_trim(&m[2]).to_string()))
}

/// Folder name "Artist - Album" or "Artist《Album》..." -> (artist, album); empty when neither.
pub fn folder_artist_album(folder: &str) -> (String, String) {
    static R: OnceLock<Regex> = OnceLock::new();
    if let Some(r) = split_dash(folder) {
        return r;
    }
    if let Some(m) = re(&R, r"^(?:\d+_)?([^《]+)《(.+?)》").captures(folder) {
        return (php_trim(&m[1]).to_string(), php_trim(&m[2]).to_string());
    }
    (String::new(), String::new())
}

pub fn derive(rel: &str, tag: &RawTags, duration: f64) -> Derived {
    static EXT: OnceLock<Regex> = OnceLock::new();
    static NUM: OnceLock<Regex> = OnceLock::new();
    static TNUM: OnceLock<Regex> = OnceLock::new();
    let file = basename(rel);
    let base = re(&EXT, r"\.[^.]+$").replace(file, "").into_owned();
    let folder = match rel.rfind('/') {
        Some(i) => basename(&rel[..i]),
        None => "",
    };

    let mut track = tag.track;
    if track == 0 {
        if let Some(m) = re(&NUM, r"^(\d{1,3})[\s._-]+").captures(&base) {
            track = m[1].parse().unwrap_or(0);
        }
    }
    let plain = re(&NUM, r"^(\d{1,3})[\s._-]+").replace(&base, "").into_owned();

    let (mut fa, mut ft) = split_dash(&plain).unwrap_or_default();
    if is_unknown(&fa) { fa.clear(); }
    if is_unknown(&ft) { ft.clear(); }

    let tt0 = clean_tag(&tag.title);
    let tt = re(&TNUM, r"^\d{1,3}[\s.]+").replace(&tt0, "").into_owned();
    let ta = or(clean_tag(&tag.artist), clean_tag(&tag.albumartist));
    let tb = clean_tag(&tag.album);

    let (da, db) = folder_artist_album(folder);

    let mut title = if !ft.is_empty() {
        ft
    } else if !tt.is_empty() && !(plain != tt && plain.starts_with(&tt)) && !is_unknown(&plain) {
        tt
    } else if !is_unknown(&plain) {
        plain.clone()
    } else {
        String::new()
    };
    let mut artist = or(fa.clone(), or(ta, da.clone()));
    if fa.is_empty() && !artist.is_empty() && !title.is_empty() && artist.starts_with(&title) {
        artist = da;
    }

    let mut album = if !db.is_empty() { db } else if !tb.is_empty() { tb } else { folder.to_string() };
    if folder.is_empty() && album == title {
        album.clear();
    }

    if duration > 1200.0 && !album.is_empty() && (title.is_empty() || title == album) {
        title = format!("《{album}》整轨");
    }
    if title.is_empty() {
        title = plain;
    }
    Derived { title, artist, album, track }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn d(rel: &str, t: &str, a: &str, b: &str, dur: f64) -> (String, String, String, u32) {
        let tag = RawTags { title: t.into(), artist: a.into(), album: b.into(), ..Default::default() };
        let r = derive(rel, &tag, dur);
        (r.title, r.artist, r.album, r.track)
    }
    fn s4(t: &str, a: &str, b: &str, n: u32) -> (String, String, String, u32) {
        (t.into(), a.into(), b.into(), n)
    }
    #[test]
    fn derive_cases() {
        assert_eq!(d("Beyond - 海阔天空.wma", "", "", "", 240.0), s4("海阔天空", "Beyond", "", 0));
        assert_eq!(d("周杰伦 - 叶惠美/01 晴天.mp3", "", "", "", 269.0), s4("晴天", "周杰伦", "叶惠美", 1));
        assert_eq!(d("散装/03 - 晴天.mp3", "晴天", "周杰伦", "", 269.0), s4("晴天", "周杰伦", "散装", 3));
        assert_eq!(d("song.mp3", "Fix You", "Coldplay", "X&Y", 295.0), s4("Fix You", "Coldplay", "X&Y", 0));
        assert_eq!(d("示例歌手《晴天集》首版 典藏[整轨WAV]/未知艺术家 - 未知标题.wav", "", "", "", 3000.0), s4("《晴天集》整轨", "示例歌手", "晴天集", 0));
        assert_eq!(d("281公里.mp3", "", "", "", 269.0), s4("281公里", "", "", 0));
        assert_eq!(d("x.mp3", "晴天", "晴天 周杰伦", "晴天", 200.0), s4("晴天", "", "", 0));
        assert_eq!(d("a.mp3", "Track 01", "未知艺术家", "www.abc.com", 200.0), s4("a", "", "", 0));
    }
    #[test]
    fn unicode_whitespace_and_digits() {
        // expected values from running the PHP derive()
        assert_eq!(d("歌手\u{3000}-\u{3000}歌名.mp3", "", "", "", 200.0), s4("歌名", "歌手", "", 0));
        assert_eq!(d("01\u{3000}晴天.mp3", "", "", "", 200.0), s4("晴天", "", "", 1));
        assert_eq!(d("０１ 晴天.mp3", "", "", "", 200.0), s4("晴天", "", "", 0));
        assert_eq!(d("专辑\u{3000}-\u{3000}x/０１ 晴天.mp3", "", "", "", 200.0), s4("晴天", "专辑", "x", 0));
        assert_eq!(d("a.mp3", "３ 标题", "", "", 200.0), s4("标题", "", "", 0));
    }
    #[test]
    fn folder_cases() {
        assert_eq!(folder_artist_album("周杰伦 - 叶惠美"), ("周杰伦".into(), "叶惠美".into()));
        assert_eq!(folder_artist_album("散装"), ("".into(), "".into()));
    }
}
