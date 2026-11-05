use super::Track;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const KEYWORDS: &[&str] = &[
    "live", "remaster", "remastered", "feat", "ft.", "with", "version", "ver.", "edit", "mix",
    "remix", "mono", "stereo", "acoustic", "demo", "bonus", "deluxe", "explicit", "现场", "版",
];

fn half_width(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .map(half_width)
        .flat_map(|c| c.to_lowercase())
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn has_keyword(s: &str) -> bool {
    let l = s.to_lowercase();
    KEYWORDS.iter().any(|k| l.contains(k))
}

fn close_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '（' => Some('）'),
        '【' => Some('】'),
        _ => None,
    }
}

pub fn norm_title(s: &str) -> String {
    // 1. drop bracketed groups containing a keyword
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some(close) = close_of(chars[i]) {
            if let Some(off) = chars[i + 1..].iter().position(|&c| c == close) {
                let inner: String = chars[i + 1..i + 1 + off].iter().collect();
                if has_keyword(&inner) {
                    i += off + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    // 2. drop " - suffix" containing a keyword
    let mut cut = out.len();
    for (idx, _) in out.match_indices(" - ") {
        if has_keyword(&out[idx + 3..]) {
            cut = idx;
            break;
        }
    }
    norm(&out[..cut])
}

pub fn split_artists(s: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\s(?:feat\.|ft\.|featuring|x)\s").unwrap());
    let s: String = s.chars().map(half_width).collect();
    let s = re.replace_all(&s, ",");
    s.split([',', ';', '/', '&', '、'])
        .map(norm)
        .filter(|a| !a.is_empty())
        .collect()
}

pub struct LibSong<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub duration: f64,
    pub quality: u32,
}

struct Entry<'a> {
    id: &'a str,
    artists: HashSet<String>,
    duration: f64,
    quality: u32,
}

pub struct Matcher<'a> {
    by_title: HashMap<String, Vec<Entry<'a>>>,
}

impl<'a> Matcher<'a> {
    pub fn new(songs: Vec<LibSong<'a>>) -> Self {
        let mut by_title: HashMap<String, Vec<Entry<'a>>> = HashMap::new();
        for s in songs {
            let key = norm_title(s.title);
            if key.is_empty() {
                continue;
            }
            by_title.entry(key).or_default().push(Entry {
                id: s.id,
                artists: split_artists(s.artist).into_iter().collect(),
                duration: s.duration,
                quality: s.quality,
            });
        }
        Matcher { by_title }
    }

    pub fn find(&self, t: &Track) -> Option<&'a str> {
        let cands = self.by_title.get(&norm_title(&t.title))?;
        let want: HashSet<String> = t.artists.iter().flat_map(|a| split_artists(a)).collect();
        let diff = |e: &Entry| match t.duration_ms {
            Some(ms) => (e.duration - ms as f64 / 1000.0).abs(),
            None => 0.0,
        };
        let best = cands
            .iter()
            .filter(|e| !e.artists.is_disjoint(&want))
            .min_by(|a, b| {
                diff(a)
                    .total_cmp(&diff(b))
                    .then(b.quality.cmp(&a.quality))
                    .then(a.id.cmp(b.id))
            });
        if let Some(e) = best {
            return Some(e.id);
        }
        t.duration_ms?;
        let mut close = cands.iter().filter(|e| diff(e) <= 2.0);
        match (close.next(), close.next()) {
            (Some(e), None) => Some(e.id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Track;

    fn tr(title: &str, artist: &str, ms: Option<u64>) -> Track {
        Track { title: title.into(), artists: vec![artist.into()], album: String::new(), duration_ms: ms }
    }
    fn ls<'a>(id: &'a str, title: &'a str, artist: &'a str, d: f64) -> LibSong<'a> {
        LibSong { id, title, artist, duration: d, quality: 0 }
    }

    #[test]
    fn norm_title_strips_versions() {
        let y = norm_title("Yesterday");
        assert_eq!(norm_title("Yesterday - Remastered 2009"), y);
        let q = norm_title("晴天");
        for s in ["晴天 (Live)", "晴天（现场版）", "晴天【Live】"] {
            assert_eq!(norm_title(s), q, "{s}");
        }
        assert_ne!(norm_title("Song (Love Song)"), norm_title("Song"));
        assert_eq!(norm_title("ＡＢＣ"), "abc");
    }

    #[test]
    fn split_artists_variants() {
        let f = norm_title("费玉清");
        for s in ["周杰伦/费玉清", "周杰伦 & 费玉清", "周杰伦、费玉清", "Jay Chou feat. 费玉清"] {
            assert!(split_artists(s).contains(&f), "{s}");
        }
    }

    #[test]
    fn matcher_prefers_artist_then_duration() {
        let m = Matcher::new(vec![
            ls("1", "晴天", "周杰伦", 269.0),
            ls("2", "晴天", "别人", 269.0),
            ls("3", "晴天 (Live)", "周杰伦", 300.0),
        ]);
        assert_eq!(m.find(&tr("晴天", "周杰伦", Some(269000))), Some("1"));
        assert_eq!(m.find(&tr("晴天", "周杰伦", Some(299000))), Some("3"));
    }

    #[test]
    fn matcher_title_only_needs_unique_close_duration() {
        let m = Matcher::new(vec![ls("1", "Hello", "X", 100.0), ls("2", "Hello", "Y", 110.0)]);
        assert_eq!(m.find(&tr("Hello", "Z", Some(101500))), Some("1"));
        assert_eq!(m.find(&tr("Hello", "Z", None)), None);
        let m2 = Matcher::new(vec![ls("1", "Hello", "X", 100.0), ls("2", "Hello", "Y", 101.0)]);
        assert_eq!(m2.find(&tr("Hello", "Z", Some(100500))), None);
    }
}
