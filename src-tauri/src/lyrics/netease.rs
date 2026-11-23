//! NetEase Cloud Music provider. Everything NetEase-specific lives in this file.
use super::online::{http_get, http_post_form, pick_auto, Candidate, NetError, Provider, Query, Raw};
use serde_json::Value;

const HEADERS: [(&str, &str); 2] = [("User-Agent", "Mozilla/5.0"), ("Referer", "https://music.163.com/")];

pub struct Netease;

pub fn parse_search(json: &str) -> Vec<Candidate> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let Some(songs) = v.get("result").and_then(|r| r.get("songs")).and_then(Value::as_array) else {
        return Vec::new();
    };
    songs
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_i64()?.to_string();
            let artist = s
                .get("ar")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.get("name")?.as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            Some(Candidate {
                source: "netease".into(),
                ext_id: id,
                title: s.get("name").and_then(Value::as_str).unwrap_or_default().into(),
                artist,
                album: s.get("al").and_then(|a| a.get("name")).and_then(Value::as_str).unwrap_or_default().into(),
                dur: s.get("dt").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0,
                synced: true,
            })
        })
        .collect()
}

/// False when `result` is present but not an object, or `songs` is present but not an array
/// (e.g. encrypted responses); such replies must not be read as "no results".
pub fn search_shape_ok(json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return false;
    };
    match v.get("result") {
        None | Some(Value::Null) => true,
        Some(Value::Object(o)) => matches!(o.get("songs"), None | Some(Value::Null) | Some(Value::Array(_))),
        Some(_) => false,
    }
}

fn lyric_of(v: &Value, k: &str) -> Option<String> {
    v.get(k)?
        .get("lyric")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// A single-line "pure music" placeholder counts as no lyrics.
fn is_placeholder(s: &str) -> bool {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.len() <= 1 && s.contains("纯音乐")
}

pub fn parse_lyric(id: &str, json: &str) -> Option<Raw> {
    let v: Value = serde_json::from_str(json).ok()?;
    if v.get("nolyric").and_then(Value::as_bool) == Some(true)
        || v.get("uncollected").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let lrc = lyric_of(&v, "lrc").filter(|s| !is_placeholder(s));
    let yrc = lyric_of(&v, "yrc");
    let trans = lyric_of(&v, "tlyric");
    if lrc.is_none() && yrc.is_none() {
        return None;
    }
    Some(Raw { source: "netease".into(), ext_id: id.into(), lrc, yrc, trans, plain: None })
}

fn checked(body: &str) -> Result<Value, NetError> {
    let v: Value = serde_json::from_str(body).map_err(|_| NetError)?;
    match v.get("code").and_then(Value::as_i64) {
        Some(200) => Ok(v),
        _ => Err(NetError),
    }
}

impl Provider for Netease {
    fn name(&self) -> &'static str {
        "netease"
    }

    fn best(&self, q: &Query) -> Result<Option<Raw>, NetError> {
        match pick_auto(q, &|x| self.search(x))? {
            Some(c) => self.fetch(&c.ext_id),
            None => Ok(None),
        }
    }

    fn search(&self, q: &Query) -> Result<Vec<Candidate>, NetError> {
        let s = format!("{} {}", q.title, q.artist);
        let form = [("s", s.trim()), ("type", "1"), ("limit", "10"), ("offset", "0")];
        let body = http_post_form("https://music.163.com/api/cloudsearch/pc", &form, &HEADERS)?.ok_or(NetError)?;
        checked(&body)?;
        if !search_shape_ok(&body) {
            return Err(NetError);
        }
        Ok(parse_search(&body))
    }

    fn fetch(&self, ext_id: &str) -> Result<Option<Raw>, NetError> {
        if ext_id.is_empty() || !ext_id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        let params = [("id", ext_id), ("lv", "1"), ("tv", "1"), ("yv", "1")];
        let Some(body) = http_get("https://music.163.com/api/song/lyric", &params, &HEADERS)? else {
            return Ok(None);
        };
        checked(&body)?;
        Ok(parse_lyric(ext_id, &body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netease_search_uses_ar_and_dt() {
        let j = r#"{"code":200,"result":{"songs":[{"id":1,"name":"晴天","ar":[{"name":"周杰伦"}],"al":{"name":"叶惠美"},"dt":269000}]}}"#;
        let c = parse_search(j);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].title.as_str(), c[0].artist.as_str(), c[0].album.as_str()), ("晴天", "周杰伦", "叶惠美"));
        assert_eq!(c[0].dur, 269.0);
        assert_eq!(c[0].ext_id, "1");
    }

    #[test]
    fn netease_parses_lrc_trans_yrc() {
        let j = r#"{"code":200,"lrc":{"lyric":"[00:01.00]a"},"tlyric":{"lyric":"[00:01.00]b"},"yrc":{"lyric":"[1000,500](1000,250,0)a"}}"#;
        let r = parse_lyric("9", j).unwrap();
        assert_eq!(r.source, "netease");
        assert_eq!(r.ext_id, "9");
        assert_eq!(r.lrc.as_deref(), Some("[00:01.00]a"));
        assert_eq!(r.trans.as_deref(), Some("[00:01.00]b"));
        assert!(r.yrc.is_some());
        assert!(parse_lyric("9", r#"{"code":200,"nolyric":true}"#).is_none());
        assert!(parse_lyric("9", r#"{"code":200,"lrc":{"lyric":""},"tlyric":{"lyric":""}}"#).is_none());
        assert!(parse_lyric("9", r#"{"code":200,"lrc":{"lyric":"[00:00.00]纯音乐，请欣赏"}}"#).is_none());
    }

    #[test]
    fn search_shape_checks() {
        assert!(!search_shape_ok(r#"{"code":200,"result":"35b17489"}"#));
        assert!(!search_shape_ok(r#"{"code":200,"result":[]}"#));
        assert!(!search_shape_ok(r#"{"code":200,"result":{"songs":"x"}}"#));
        assert!(search_shape_ok(r#"{"code":200,"result":{}}"#));
        assert!(search_shape_ok(r#"{"code":200,"result":{"songs":[]}}"#));
        assert!(search_shape_ok(r#"{"code":200,"result":{"songCount":0}}"#));
    }

    #[test]
    fn best_without_artist_makes_no_request() {
        let q = Query { title: "A".into(), artist: " ".into(), album: String::new(), duration: 100.0 };
        assert_eq!(Netease.best(&q), Ok(None));
    }

    #[test]
    fn netease_encrypted_result_is_empty() {
        assert!(parse_search(r#"{"code":200,"result":"35b17489"}"#).is_empty());
    }
}

#[cfg(test)]
mod live {
    use super::super::lrclib::Lrclib;
    use super::*;

    #[test]
    #[ignore]
    fn live_smoke() {
        let q = Query { title: "Yesterday".into(), artist: "The Beatles".into(), album: String::new(), duration: 0.0 };
        for p in [&Lrclib as &dyn Provider, &Netease as &dyn Provider] {
            let r = p.best(&q);
            println!("{}: {:?}", p.name(), r.as_ref().map(|o| o.as_ref().map(|r| (r.lrc.as_ref().map(|s| s.len()), r.yrc.is_some()))));
            let raw = r.expect("net").expect("some lyrics");
            assert!(raw.lrc.as_deref().is_some_and(|l| l.contains("[0")), "{} lacks timed lyrics", p.name());
        }
    }
}
