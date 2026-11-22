//! Pure lyric parsing: LRC, enhanced LRC and NetEase yrc. Times are milliseconds.

use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    pub start: u32,
    pub end: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    pub start: Option<u32>,
    pub end: Option<u32>,
    pub text: String,
    pub words: Vec<Word>,
    pub trans: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parsed {
    pub lines: Vec<Line>,
}

impl Parsed {
    pub fn kind(&self) -> &'static str {
        if self.lines.iter().any(|l| !l.words.is_empty()) {
            "word"
        } else if self.lines.iter().any(|l| l.start.is_some()) {
            "line"
        } else {
            "plain"
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

const TIME: &str = r"(\d+):(\d{1,2})(?:[.:](\d{1,3}))?";

fn time_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!("^{TIME}$")).unwrap())
}
fn marker_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!("<{TIME}>")).unwrap())
}
fn tag_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[A-Za-z#]+\s*:").unwrap())
}
fn yrc_head_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\[(\d+),(\d+)\]").unwrap())
}
fn yrc_word_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\((\d+),(\d+),-?\d+\)").unwrap())
}

/// `m:ss[.x|.xx|.xxx|:xx]` captures -> ms; None on overflow.
fn time_ms(c: &regex::Captures) -> Option<i64> {
    let min: u64 = c[1].parse().ok()?;
    let sec: u64 = c[2].parse().ok()?;
    let frac = match c.get(3) {
        None => 0,
        Some(f) => {
            let n: u64 = f.as_str().parse().ok()?;
            match f.as_str().len() {
                1 => n * 100,
                2 => n * 10,
                _ => n,
            }
        }
    };
    let ms = min
        .checked_mul(60_000)?
        .checked_add(sec.checked_mul(1000)?)?
        .checked_add(frac)?;
    u32::try_from(ms).ok().map(i64::from)
}

struct RawWord {
    start: i64,
    end: Option<i64>,
    text: String,
}

struct Raw {
    start: i64,
    text: String,
    words: Vec<RawWord>,
}

fn clamp(t: i64, offset: i64) -> u32 {
    u32::try_from((t - offset).max(0)).unwrap_or(u32::MAX)
}

/// Split a line body on `<time>` markers into words (None when no valid markers).
fn parse_words(body: &str, first_stamp: i64) -> Option<(String, Vec<RawWord>)> {
    let caps: Vec<_> = marker_re().captures_iter(body).collect();
    if caps.is_empty() {
        return None;
    }
    let mut times = Vec::with_capacity(caps.len());
    for c in &caps {
        times.push(time_ms(c)?);
    }
    let mut words = Vec::new();
    let lead = body[..caps[0].get(0).unwrap().start()].trim();
    if !lead.is_empty() {
        words.push(RawWord { start: first_stamp, end: Some(times[0]), text: lead.to_string() });
    }
    for (i, c) in caps.iter().enumerate() {
        let from = c.get(0).unwrap().end();
        let to = caps.get(i + 1).map_or(body.len(), |n| n.get(0).unwrap().start());
        let text = &body[from..to];
        if text.trim().is_empty() {
            continue;
        }
        words.push(RawWord { start: times[i], end: times.get(i + 1).copied(), text: text.to_string() });
    }
    let text: String = words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string();
    Some((text, words))
}

pub fn parse_lrc(text: &str) -> Parsed {
    let text = text.trim_start_matches('\u{feff}');
    let mut offset = 0i64;
    let mut any_stamp = false;
    let mut raws: Vec<Raw> = Vec::new();
    let mut plain: Vec<String> = Vec::new();

    for line in text.lines() {
        let mut rest = line.trim();
        let mut stamps: Vec<i64> = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let inner = &rest[1..end];
            if let Some(c) = time_re().captures(inner) {
                any_stamp = true;
                if let Some(t) = time_ms(&c) {
                    stamps.push(t);
                }
            } else if tag_re().is_match(inner) {
                if let Some(v) = inner.strip_prefix("offset:") {
                    if let Ok(o) = v.trim().trim_start_matches('+').parse::<i64>() {
                        offset = o;
                    }
                }
            } else {
                break;
            }
            rest = rest[end + 1..].trim_start();
        }
        let body = rest.trim();
        if stamps.is_empty() {
            if !body.is_empty() {
                plain.push(body.to_string());
            }
            continue;
        }
        let (line_text, words) = match parse_words(body, stamps[0]) {
            Some(x) => x,
            None => (marker_re().replace_all(body, "").trim().to_string(), Vec::new()),
        };
        if line_text.is_empty() {
            continue;
        }
        for &s in &stamps {
            let delta = s - stamps[0];
            raws.push(Raw {
                start: s,
                text: line_text.clone(),
                words: words
                    .iter()
                    .map(|w| RawWord { start: w.start + delta, end: w.end.map(|e| e + delta), text: w.text.clone() })
                    .collect(),
            });
        }
    }

    if !any_stamp {
        return Parsed {
            lines: plain
                .into_iter()
                .map(|t| Line { start: None, end: None, text: t, words: Vec::new(), trans: None })
                .collect(),
        };
    }

    raws.sort_by_key(|r| clamp(r.start, offset));
    let starts: Vec<u32> = raws.iter().map(|r| clamp(r.start, offset)).collect();
    let lines = raws
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let end = starts.get(i + 1).copied();
            let words = r
                .words
                .into_iter()
                .map(|w| {
                    let s = clamp(w.start, offset);
                    let e = w.end.map(|e| clamp(e, offset)).or(end).unwrap_or(s).max(s);
                    Word { start: s, end: e, text: w.text }
                })
                .collect();
            Line { start: Some(starts[i]), end, text: r.text, words, trans: None }
        })
        .collect();
    Parsed { lines }
}

pub fn parse_yrc(text: &str) -> Parsed {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('{') {
            continue;
        }
        let Some(h) = yrc_head_re().captures(line) else { continue };
        let (Ok(start), Ok(dur)) = (h[1].parse::<u32>(), h[2].parse::<u32>()) else { continue };
        let end = start.saturating_add(dur);
        let body = &line[h.get(0).unwrap().end()..];
        let caps: Vec<_> = yrc_word_re().captures_iter(body).collect();
        let mut words = Vec::new();
        for (i, c) in caps.iter().enumerate() {
            let from = c.get(0).unwrap().end();
            let to = caps.get(i + 1).map_or(body.len(), |n| n.get(0).unwrap().start());
            let t = &body[from..to];
            let (Ok(ws), Ok(wd)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) else { continue };
            if t.trim().is_empty() {
                continue;
            }
            words.push(Word { start: ws, end: ws.saturating_add(wd), text: t.to_string() });
        }
        let text: String = if caps.is_empty() {
            body.trim().to_string()
        } else {
            words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string()
        };
        if text.is_empty() {
            continue;
        }
        lines.push(Line { start: Some(start), end: Some(end), text, words, trans: None });
    }
    lines.sort_by_key(|l: &Line| l.start);
    Parsed { lines }
}

pub fn merge_translation(base: &mut Parsed, trans: &Parsed) {
    for line in &mut base.lines {
        let Some(s) = line.start else { continue };
        let best = trans
            .lines
            .iter()
            .filter_map(|t| t.start.map(|ts| (ts.abs_diff(s), t)))
            .filter(|(d, _)| *d <= 300)
            .min_by_key(|(d, _)| *d);
        if let Some((_, t)) = best {
            line.trans = Some(t.text.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(p: &Parsed) -> Vec<(Option<u32>, &str)> {
        p.lines.iter().map(|l| (l.start, l.text.as_str())).collect()
    }

    #[test]
    fn basic_lrc() {
        let p = parse_lrc("[00:01.50]甲\n[00:03.00]乙");
        let starts: Vec<_> = p.lines.iter().map(|l| l.start).collect();
        assert_eq!(starts, vec![Some(1500), Some(3000)]);
        assert_eq!(p.lines[0].end, Some(3000));
        assert_eq!(p.lines[1].end, None);
        assert_eq!(p.kind(), "line");
    }

    #[test]
    fn stamp_formats() {
        for (s, ms) in [("[1:02]", 62000), ("[01:02.5]", 62500), ("[01:02.50]", 62500), ("[01:02.500]", 62500), ("[01:02:50]", 62500)] {
            let p = parse_lrc(&format!("{s}x"));
            assert_eq!(p.lines[0].start, Some(ms), "{s}");
        }
    }

    #[test]
    fn multi_stamp_and_unsorted() {
        let p = parse_lrc("[00:05.00][00:01.00]副歌\n[00:03.00]主歌");
        assert_eq!(pairs(&p), vec![(Some(1000), "副歌"), (Some(3000), "主歌"), (Some(5000), "副歌")]);
    }

    #[test]
    fn offset_tag_applied() {
        let p = parse_lrc("[offset:+500]\n[00:01.00]甲\n[00:00.20]乙");
        assert_eq!(pairs(&p), vec![(Some(0), "乙"), (Some(500), "甲")]);
    }

    #[test]
    fn metadata_and_blank_dropped() {
        let p = parse_lrc("[ti:x]\n[ar:y]\n[00:01.00]\n[00:02.00]甲\n[by:z]");
        assert_eq!(pairs(&p), vec![(Some(2000), "甲")]);
    }

    #[test]
    fn plain_text() {
        let p = parse_lrc("[ti:x]\n第一行\n\n第二行");
        assert_eq!(p.lines.len(), 2);
        assert!(p.lines.iter().all(|l| l.start.is_none()));
        assert_eq!(p.kind(), "plain");
    }

    #[test]
    fn enhanced_lrc_words() {
        let p = parse_lrc("[00:01.00]<00:01.00>你<00:01.50>好<00:02.00>\n[00:03.00]下");
        assert_eq!(p.lines[0].text, "你好");
        let w: Vec<_> = p.lines[0].words.iter().map(|w| (w.start, w.end, w.text.as_str())).collect();
        assert_eq!(w, vec![(1000, 1500, "你"), (1500, 2000, "好")]);
        assert_eq!(p.kind(), "word");
    }

    #[test]
    fn enhanced_last_word_uses_line_end() {
        let p = parse_lrc("[00:01.00]<00:01.00>你<00:01.50>好\n[00:03.00]下");
        assert_eq!(p.lines[0].words[1].end, 3000);
    }

    #[test]
    fn yrc_words() {
        let p = parse_yrc("[1000,1000](1000,400,0)你(1400,600,0)好");
        let l = &p.lines[0];
        assert_eq!((l.start, l.end), (Some(1000), Some(2000)));
        let w: Vec<_> = l.words.iter().map(|w| (w.start, w.end, w.text.as_str())).collect();
        assert_eq!(w, vec![(1000, 1400, "你"), (1400, 2000, "好")]);
    }

    #[test]
    fn yrc_ignores_json_credit_lines() {
        let p = parse_yrc("{\"t\":0,\"c\":[{\"tx\":\"作词: x\"}]}\n[1000,500](1000,500,0)甲");
        assert_eq!(p.lines.len(), 1);
        assert_eq!(p.lines[0].text, "甲");
    }

    #[test]
    fn translation_merges_by_time() {
        let mut base = parse_lrc("[00:01.00]a\n[00:03.00]b");
        let trans = parse_lrc("[00:01.10]甲\n[00:09.00]乙");
        merge_translation(&mut base, &trans);
        assert_eq!(base.lines[0].trans.as_deref(), Some("甲"));
        assert_eq!(base.lines[1].trans, None);
    }

    #[test]
    fn garbage_never_panics() {
        let _ = parse_lrc("[99999999999:99.99]x\n[:]\n[00:00");
        let _ = parse_yrc("[99999999999,1](1,2,3\n[1,2](x");
    }
}
