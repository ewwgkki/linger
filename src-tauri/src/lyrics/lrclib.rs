use super::online::{http_get, Candidate, NetError, Provider, Query, Raw};
use serde_json::Value;

const UA: &str = "Linger/0.1 (https://github.com/ewwgkki/linger)";
const BASE: &str = "https://lrclib.net/api";

pub struct Lrclib;

fn nonempty(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

fn id_of(v: &Value) -> String {
    match v.get("id") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

pub fn parse_track(json: &str) -> Option<Raw> {
    let v: Value = serde_json::from_str(json).ok()?;
    if !v.is_object() {
        return None;
    }
    let lrc = nonempty(&v, "syncedLyrics");
    let plain = nonempty(&v, "plainLyrics");
    if lrc.is_none() && plain.is_none() {
        return None;
    }
    Some(Raw { source: "lrclib".into(), ext_id: id_of(&v), lrc, plain, ..Default::default() })
}

pub fn parse_search(json: &str) -> Vec<Candidate> {
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    arr.iter()
        .filter(|v| v.is_object())
        .map(|v| Candidate {
            source: "lrclib".into(),
            ext_id: id_of(v),
            title: nonempty(v, "trackName").unwrap_or_default(),
            artist: nonempty(v, "artistName").unwrap_or_default(),
            album: nonempty(v, "albumName").unwrap_or_default(),
            dur: v.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
            synced: nonempty(v, "syncedLyrics").is_some(),
        })
        .collect()
}

/// 200 bodies must be a JSON object (track) or array (search); anything else is a failure.
fn shape_ok(s: &str, array: bool) -> Result<(), NetError> {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Array(_)) if array => Ok(()),
        Ok(Value::Object(_)) if !array => Ok(()),
        _ => Err(NetError),
    }
}

/// LRCLIB rejects empty fields: a blank artist is left out, a blank title searches by artist.
fn search_params(q: &Query) -> Vec<(&'static str, &str)> {
    let (title, artist) = (q.title.trim(), q.artist.trim());
    if title.is_empty() {
        return vec![("q", artist)];
    }
    let mut params = vec![("track_name", title)];
    if !artist.is_empty() {
        params.push(("artist_name", artist));
    }
    params
}

impl Provider for Lrclib {
    fn name(&self) -> &'static str {
        "lrclib"
    }

    fn best(&self, q: &Query) -> Result<Option<Raw>, NetError> {
        let mut exact = None;
        if super::online::has_artist(q) {
            let dur = (q.duration.round() as i64).to_string();
            let mut params = vec![("track_name", q.title.as_str()), ("artist_name", q.artist.as_str())];
            if !q.album.is_empty() {
                params.push(("album_name", q.album.as_str()));
            }
            if q.duration > 0.0 {
                params.push(("duration", dur.as_str()));
            }
            if let Some(body) = http_get(&format!("{BASE}/get"), &params, &[("User-Agent", UA)])? {
                shape_ok(&body, false)?;
                exact = parse_track(&body);
                if exact.as_ref().is_some_and(|raw| raw.lrc.is_some()) {
                    return Ok(exact);
                }
            }
        }
        // no exact timed entry: search and match on title and length
        match (super::online::pick_auto(q, &|x| self.search(x)), exact) {
            (Ok(Some(c)), _) if c.synced => self.fetch(&c.ext_id),
            (_, Some(raw)) => Ok(Some(raw)),
            (Ok(Some(c)), None) => self.fetch(&c.ext_id),
            (Ok(None), None) => Ok(None),
            (Err(e), None) => Err(e),
        }
    }

    fn search(&self, q: &Query) -> Result<Vec<Candidate>, NetError> {
        let params = search_params(q);
        match http_get(&format!("{BASE}/search"), &params, &[("User-Agent", UA)])? {
            Some(body) => {
                shape_ok(&body, true)?;
                Ok(parse_search(&body))
            }
            None => Ok(Vec::new()),
        }
    }

    fn fetch(&self, ext_id: &str) -> Result<Option<Raw>, NetError> {
        if ext_id.is_empty() || !ext_id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        match http_get(&format!("{BASE}/get/{ext_id}"), &[], &[("User-Agent", UA)])? {
            Some(body) => {
                shape_ok(&body, false)?;
                Ok(parse_track(&body))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrclib_parses_track() {
        let j = r#"{"id":5,"trackName":"A","artistName":"B","albumName":"C","duration":200.4,"instrumental":false,"plainLyrics":"x\ny","syncedLyrics":"[00:01.00]x"}"#;
        let r = parse_track(j).unwrap();
        assert_eq!(r.source, "lrclib");
        assert_eq!(r.ext_id, "5");
        assert_eq!(r.lrc.as_deref(), Some("[00:01.00]x"));
        assert_eq!(r.plain.as_deref(), Some("x\ny"));
    }

    #[test]
    fn lrclib_404_style_empty_is_none() {
        assert!(parse_track(r#"{"code":404,"name":"NotFound","message":"x"}"#).is_none());
        assert!(parse_track(r#"{"id":1,"instrumental":true,"plainLyrics":null,"syncedLyrics":""}"#).is_none());
        assert!(parse_track("not json").is_none());
    }

    #[test]
    fn lrclib_wrong_shape_is_error() {
        assert_eq!(shape_ok("[]", false), Err(NetError));
        assert_eq!(shape_ok("{}", true), Err(NetError));
        assert_eq!(shape_ok("\"x\"", true), Err(NetError));
        assert_eq!(shape_ok("{}", false), Ok(()));
        assert_eq!(shape_ok("[]", true), Ok(()));
    }

    fn query(title: &str, artist: &str) -> Query {
        Query { title: title.into(), artist: artist.into(), album: String::new(), duration: 100.0 }
    }

    #[test]
    fn search_omits_blank_fields() {
        assert_eq!(search_params(&query("A", "B")), vec![("track_name", "A"), ("artist_name", "B")]);
        assert_eq!(search_params(&query("A", "  ")), vec![("track_name", "A")]);
        assert_eq!(search_params(&query("", "B")), vec![("q", "B")]);
    }

    #[test]
    fn best_without_artist_makes_no_request() {
        assert_eq!(Lrclib.best(&query("A", "")), Ok(None));
    }

    #[test]
    fn lrclib_search_list() {
        let j = r#"[{"id":1,"trackName":"A","artistName":"B","albumName":"C","duration":200,"syncedLyrics":"[00:01.00]x","plainLyrics":"x"},{"id":2,"trackName":"A","artistName":"B","albumName":null,"duration":201.5,"syncedLyrics":null,"plainLyrics":"x"}]"#;
        let c = parse_search(j);
        assert_eq!(c.len(), 2);
        assert!(c[0].synced && !c[1].synced);
        assert_eq!(c[1].dur, 201.5);
        assert_eq!(c[0].ext_id, "1");
        assert!(parse_search("{}").is_empty());
    }
}
