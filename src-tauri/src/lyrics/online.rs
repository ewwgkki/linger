//! Shared types, HTTP plumbing and strict auto-match for online lyric providers.
use crate::import::matcher::{norm_title, split_artists};
use serde::Serialize;
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

const MAX_BODY: u64 = 2 * 1024 * 1024;
const NO_ARTIST_TOLERANCE: f64 = 2.0;
const MISMATCH_TOLERANCE: f64 = 1.5;
const SHORT_TITLE_TOLERANCE: f64 = 1.0;

#[derive(Debug, Clone)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Raw {
    pub source: String,
    pub ext_id: String,
    pub lrc: Option<String>,
    pub yrc: Option<String>,
    pub trans: Option<String>,
    pub plain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub source: String,
    pub ext_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub dur: f64,
    pub synced: bool,
}

#[derive(Debug, PartialEq)]
pub struct NetError;

pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn best(&self, q: &Query) -> Result<Option<Raw>, NetError>;
    fn search(&self, q: &Query) -> Result<Vec<Candidate>, NetError>;
    fn fetch(&self, ext_id: &str) -> Result<Option<Raw>, NetError>;
}

fn agent() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
    })
}

fn read_body(resp: ureq::Response) -> Result<String, NetError> {
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut buf)
        .map_err(|_| NetError)?;
    if buf.len() as u64 > MAX_BODY {
        return Err(NetError);
    }
    String::from_utf8(buf).map_err(|_| NetError)
}

fn finish(r: Result<ureq::Response, ureq::Error>) -> Result<Option<String>, NetError> {
    match r {
        Ok(resp) => read_body(resp).map(Some),
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(_) => Err(NetError),
    }
}

/// GET; `Ok(None)` on 404.
pub fn http_get(
    url: &str,
    query: &[(&str, &str)],
    headers: &[(&str, &str)],
) -> Result<Option<String>, NetError> {
    let mut req = agent().get(url);
    for (k, v) in query {
        req = req.query(k, v);
    }
    for (k, v) in headers {
        req = req.set(k, v);
    }
    finish(req.call())
}

/// POST form; `Ok(None)` on 404.
pub fn http_post_form(
    url: &str,
    form: &[(&str, &str)],
    headers: &[(&str, &str)],
) -> Result<Option<String>, NetError> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    finish(req.send_form(form))
}

/// LRCLIB's exact lookup needs an artist.
pub fn has_artist(q: &Query) -> bool {
    !q.artist.trim().is_empty()
}

fn is_cover(title: &str) -> bool {
    let t = title.to_lowercase();
    if ["原唱", "翻唱", "翻自"].iter().any(|m| t.contains(m)) {
        return true;
    }
    let b = t.as_bytes();
    t.match_indices("cover").any(|(i, m)| {
        let before = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let after = b.get(i + m.len()).map_or(true, |c| !c.is_ascii_alphanumeric());
        before && after
    })
}

/// Strict auto-match: normalized title equal, artist sets intersect, duration within 3 s.
pub fn pick_best<'a>(cands: &'a [Candidate], q: &Query) -> Option<&'a Candidate> {
    let qt = norm_title(&q.title);
    let qa = split_artists(&q.artist);
    let mut best: Option<(&Candidate, f64)> = None;
    for c in cands {
        if is_cover(&c.title) || norm_title(&c.title) != qt {
            continue;
        }
        let ca = split_artists(&c.artist);
        if !ca.iter().any(|a| qa.contains(a)) && !same_compact(&c.artist, &q.artist) {
            continue;
        }
        let diff = (c.dur - q.duration).abs();
        if q.duration > 0.0 && diff > 3.0 {
            continue;
        }
        let diff = if q.duration > 0.0 { diff } else { 0.0 };
        let better = match best {
            None => true,
            Some((b, bd)) => {
                if c.synced != b.synced {
                    c.synced
                } else {
                    diff < bd
                }
            }
        };
        if better {
            best = Some((c, diff));
        }
    }
    best.map(|(c, _)| c)
}

/// "ImagineDragons" and "Imagine Dragons" are the same name.
fn same_compact(a: &str, b: &str) -> bool {
    let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (key(a), key(b));
    !a.is_empty() && a == b
}

/// For a missing or differently written artist (Eagles / 老鹰乐队): the title must be equal and the
/// length must agree closely, more closely still for one-character titles.
pub fn pick_relaxed<'a>(cands: &'a [Candidate], q: &Query) -> Option<&'a Candidate> {
    let qt = norm_title(&q.title);
    if qt.is_empty() || q.duration <= 0.0 {
        return None;
    }
    let base = if has_artist(q) { MISMATCH_TOLERANCE } else { NO_ARTIST_TOLERANCE };
    let tolerance = if qt.chars().count() <= 1 { base.min(SHORT_TITLE_TOLERANCE) } else { base };
    let mut best: Option<(&Candidate, f64)> = None;
    for c in cands {
        let diff = (c.dur - q.duration).abs();
        if is_cover(&c.title) || norm_title(&c.title) != qt || c.dur <= 0.0 || diff > tolerance {
            continue;
        }
        let better = best.is_none_or(|(b, bd)| if c.synced != b.synced { c.synced } else { diff < bd });
        if better {
            best = Some((c, diff));
        }
    }
    best.map(|(c, _)| c)
}

/// The automatic choice: exact match first, then title and length, then a title-only search.
pub fn pick_auto(q: &Query, search: &dyn Fn(&Query) -> Result<Vec<Candidate>, NetError>) -> Result<Option<Candidate>, NetError> {
    if q.title.trim().is_empty() {
        return Ok(None);
    }
    let first = pick_from(&search(q)?, q);
    if !has_artist(q) || first.as_ref().is_some_and(|c| c.synced) {
        return Ok(first);
    }
    // nothing timed yet: a title-only search may find the timed version
    let title_only = Query { artist: String::new(), ..q.clone() };
    let second = match search(&title_only) {
        Ok(cands) => pick_from(&cands, q),
        Err(e) if first.is_none() => return Err(e),
        Err(_) => None,
    };
    Ok(match (first, second) {
        (_, Some(s)) if s.synced => Some(s),
        (Some(f), _) => Some(f),
        (None, s) => s,
    })
}

/// A timed match if there is one, otherwise any match.
fn pick_from(cands: &[Candidate], q: &Query) -> Option<Candidate> {
    let timed: Vec<Candidate> = cands.iter().filter(|c| c.synced).cloned().collect();
    pick_best(&timed, q)
        .or_else(|| pick_relaxed(&timed, q))
        .or_else(|| pick_best(cands, q))
        .or_else(|| pick_relaxed(cands, q))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(title: &str, artist: &str, dur: f64, synced: bool, id: &str) -> Candidate {
        Candidate {
            source: "t".into(),
            ext_id: id.into(),
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            dur,
            synced,
        }
    }
    fn q(title: &str, artist: &str, dur: f64) -> Query {
        Query { title: title.into(), artist: artist.into(), album: String::new(), duration: dur }
    }

    #[test]
    fn netease_skips_cover_version() {
        let both = [cand("晴天 (原唱 周杰伦)", "RyaVocal", 270.0, true, "1"), cand("晴天", "周杰伦", 269.0, true, "2")];
        assert_eq!(pick_best(&both, &q("晴天", "周杰伦", 269.0)).unwrap().ext_id, "2");
        assert!(pick_best(&both[..1], &q("晴天", "周杰伦", 269.0)).is_none());
        let cv = [cand("晴天 (原唱 周杰伦)", "周杰伦", 269.0, true, "1")];
        assert!(pick_best(&cv, &q("晴天", "周杰伦", 269.0)).is_none());
        let en = [cand("Yesterday (Cover)", "The Beatles", 125.0, true, "1")];
        assert!(pick_best(&en, &q("Yesterday", "The Beatles", 125.0)).is_none());
    }

    #[test]
    fn cover_is_whole_word_only() {
        for t in ["Recover", "Discovery", "Undercover"] {
            let c = [cand(t, "B", 100.0, true, "1")];
            assert!(pick_best(&c, &q(t, "B", 100.0)).is_some(), "{t}");
        }
        for t in ["Song (Cover)", "Cover by X", "Song - cover"] {
            let c = [cand(t, "B", 100.0, true, "1")];
            assert!(pick_best(&c, &q(t, "B", 100.0)).is_none(), "{t}");
        }
    }

    fn stub(response: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            let _ = s.write_all(&response);
            String::from_utf8_lossy(&req).lines().next().unwrap_or("").to_string()
        });
        (url, h)
    }

    fn resp(status: &str, body: &[u8]) -> Vec<u8> {
        let mut v = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn http_statuses_and_body_cap() {
        let (u, h) = stub(resp("404 Not Found", b"{}"));
        assert_eq!(http_get(&u, &[], &[]), Ok(None));
        h.join().unwrap();
        for st in ["429 Too Many Requests", "500 Internal Server Error"] {
            let (u, h) = stub(resp(st, b"x"));
            assert_eq!(http_get(&u, &[], &[]), Err(NetError), "{st}");
            h.join().unwrap();
        }
        let (u, h) = stub(resp("200 OK", &vec![b'a'; 2 * 1024 * 1024 + 1]));
        assert_eq!(http_get(&u, &[], &[]), Err(NetError));
        let _ = h.join();
        let (u, h) = stub(resp("200 OK", b"ok"));
        assert_eq!(http_get(&u, &[], &[]), Ok(Some("ok".into())));
        h.join().unwrap();
    }

    #[test]
    fn query_is_percent_encoded() {
        let (u, h) = stub(resp("200 OK", b"ok"));
        http_get(&u, &[("track_name", "A & B 晴天")], &[]).unwrap();
        let line = h.join().unwrap();
        assert!(line.contains("track_name=A%20%26%20B%20%E6%99%B4%E5%A4%A9") || line.contains("track_name=A+%26+B+%E6%99%B4%E5%A4%A9"), "{line}");
    }

    #[test]
    fn best_requires_artist_and_duration() {
        let c = [cand("A", "B", 200.0, true, "1")];
        assert!(pick_best(&c, &q("A", "C", 200.0)).is_none());
        assert!(pick_best(&c, &q("A", "B", 205.0)).is_none());
        assert!(pick_best(&c, &q("A", "B", 202.0)).is_some());
        assert!(pick_best(&c, &q("A", "B", 0.0)).is_some());
    }

    #[test]
    fn auto_match_needs_an_artist() {
        assert!(has_artist(&q("A", "B", 1.0)));
        assert!(!has_artist(&q("A", "", 1.0)));
        assert!(!has_artist(&q("A", "  ", 1.0)));
    }

    #[test]
    fn missing_artist_matches_on_title_and_length() {
        let c = [cand("蚀月", "某歌手", 293.0, true, "1"), cand("蚀月", "另一位", 240.0, true, "2")];
        assert_eq!(pick_relaxed(&c, &q("蚀月", "", 293.4)).unwrap().ext_id, "1");
        assert!(pick_relaxed(&c, &q("蚀月", "", 280.0)).is_none());
        assert!(pick_relaxed(&c, &q("蚀月", "", 0.0)).is_none());
    }

    #[test]
    fn translated_artist_needs_a_closer_length() {
        let c = [cand("Hotel California", "老鹰乐队", 391.0, true, "1")];
        assert!(pick_relaxed(&c, &q("Hotel California", "Eagles", 392.0)).is_some());
        assert!(pick_relaxed(&c, &q("Hotel California", "Eagles", 393.0)).is_none());
    }

    #[test]
    fn one_character_titles_are_stricter() {
        let c = [cand("爱", "A", 200.0, true, "1")];
        assert!(pick_relaxed(&c, &q("爱", "", 200.9)).is_some());
        assert!(pick_relaxed(&c, &q("爱", "", 201.5)).is_none());
    }

    #[test]
    fn relaxed_match_never_takes_a_cover_or_other_title() {
        let c = [cand("晴天 (Cover)", "X", 269.0, true, "1"), cand("雨天", "X", 269.0, true, "2")];
        assert!(pick_relaxed(&c, &q("晴天", "", 269.0)).is_none());
    }

    #[test]
    fn spacing_in_an_artist_name_does_not_matter() {
        let c = [cand("Believer", "Imagine Dragons", 204.0, true, "1")];
        assert!(pick_best(&c, &q("Believer", "ImagineDragons", 204.0)).is_some());
    }

    #[test]
    fn auto_pick_falls_back_to_a_title_search() {
        use std::cell::RefCell;
        let asked = RefCell::new(Vec::new());
        let search = |x: &Query| {
            asked.borrow_mut().push(x.artist.clone());
            Ok(if x.artist.is_empty() { vec![cand("A", "译名", 200.0, true, "9")] } else { vec![] })
        };
        assert_eq!(pick_auto(&q("A", "Name", 200.0), &search).unwrap().unwrap().ext_id, "9");
        assert_eq!(*asked.borrow(), vec!["Name".to_string(), String::new()]);
    }

    #[test]
    fn auto_pick_stops_at_an_exact_match_and_reports_errors() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let found = |_: &Query| { calls.set(calls.get() + 1); Ok(vec![cand("A", "B", 200.0, true, "1")]) };
        assert_eq!(pick_auto(&q("A", "B", 200.0), &found).unwrap().unwrap().ext_id, "1");
        assert_eq!(calls.get(), 1);
        let down = |_: &Query| Err(NetError);
        assert_eq!(pick_auto(&q("A", "B", 200.0), &down), Err(NetError));
    }

    #[test]
    fn a_timed_relaxed_match_beats_an_exact_plain_one() {
        let c = [cand("A", "B", 200.0, false, "plain"), cand("A", "Other", 200.0, true, "timed")];
        let found = |_: &Query| Ok(c.to_vec());
        assert_eq!(pick_auto(&q("A", "B", 200.0), &found).unwrap().unwrap().ext_id, "timed");
    }

    #[test]
    fn plain_only_results_look_further_then_keep_the_plain_one() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let found = |_: &Query| { calls.set(calls.get() + 1); Ok(vec![cand("A", "B", 200.0, false, "1")]) };
        assert_eq!(pick_auto(&q("A", "B", 200.0), &found).unwrap().unwrap().ext_id, "1");
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn a_failed_second_search_keeps_the_first_result() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let flaky = |_: &Query| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 { Ok(vec![cand("A", "B", 200.0, false, "1")]) } else { Err(NetError) }
        };
        assert_eq!(pick_auto(&q("A", "B", 200.0), &flaky).unwrap().unwrap().ext_id, "1");
    }

    #[test]
    fn best_prefers_synced_then_closest() {
        let c = [
            cand("A", "B", 200.0, false, "1"),
            cand("A", "B", 202.0, true, "2"),
            cand("A", "B", 201.0, true, "3"),
            cand("A", "B", 201.0, true, "4"),
        ];
        assert_eq!(pick_best(&c, &q("A", "B", 200.0)).unwrap().ext_id, "3");
    }
}
