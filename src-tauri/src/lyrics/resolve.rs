//! Source order, cache and retry rules for lyrics.
use super::online::{Provider, Query, Raw};
use super::{merge_translation, parse_lrc, parse_yrc, Parsed};
use crate::db::{Db, LyricRow, SongRow};
use crate::import::matcher::norm_title;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub const NONE_TTL_SECS: i64 = 7 * 24 * 3600;
/// Bumped when auto-matching improves, so earlier "no lyrics" answers are looked up again.
const MATCH_RULES: u32 = 3;
const MAX_COMPARE_TOKENS: usize = 1500;
const MIN_COMPARE_TOKENS: usize = 8;
const SAME_LYRICS_RATIO: f64 = 0.6;

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub parsed: Parsed,
    /// "embedded" | "file" | "lrclib" | "netease" | "manual"
    pub source: Option<String>,
    /// "ok" | "none" | "retry"
    pub status: &'static str,
}

fn ok(parsed: Parsed, source: &str) -> Resolved {
    Resolved { parsed, source: Some(source.to_string()), status: "ok" }
}
fn status_only(status: &'static str) -> Resolved {
    Resolved { parsed: Parsed::default(), source: None, status }
}
fn lock(db: &Mutex<Db>) -> MutexGuard<'_, Db> {
    db.lock().unwrap_or_else(|e| e.into_inner())
}

/// yrc first, else lrc, else plain; the translation is merged in.
pub fn parsed_of(row: &LyricRow) -> Parsed {
    parse_texts(row.yrc.as_deref(), row.lrc.as_deref(), row.plain.as_deref(), row.trans.as_deref())
}

fn parse_texts(yrc: Option<&str>, lrc: Option<&str>, plain: Option<&str>, trans: Option<&str>) -> Parsed {
    let mut p = yrc.map(parse_yrc).filter(|p| !p.is_empty())
        .or_else(|| lrc.map(parse_lrc).filter(|p| !p.is_empty()))
        .or_else(|| plain.map(parse_lrc).filter(|p| !p.is_empty()))
        .unwrap_or_default();
    if let Some(t) = trans.map(parse_lrc).filter(|t| !t.is_empty()) {
        merge_translation(&mut p, &t);
    }
    p
}

/// What a "none" answer depended on: normalized tags, rounded duration, the providers asked.
fn none_key(song: &SongRow, providers: &[&dyn Provider]) -> String {
    let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
    let text = [
        norm_title(&song.title),
        song.artist.trim().to_lowercase(),
        song.album.trim().to_lowercase(),
        (song.duration.round() as i64).to_string(),
        names.join(","),
        MATCH_RULES.to_string(),
    ].join("\0");
    sha1_smol::Sha1::from(text).digest().to_string()[..16].to_string()
}

/// Words (Latin) and single characters (CJK), for comparing two versions of a lyric.
fn lyric_tokens(p: &Parsed) -> Vec<String> {
    let mut out = Vec::new();
    for line in &p.lines {
        let mut word = String::new();
        for c in line.text.chars() {
            if c.is_ascii_alphanumeric() {
                word.push(c.to_ascii_lowercase());
                continue;
            }
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            if c.is_alphanumeric() {
                out.push(c.to_string());
            }
        }
        if !word.is_empty() {
            out.push(word);
        }
        if out.len() >= MAX_COMPARE_TOKENS {
            out.truncate(MAX_COMPARE_TOKENS);
            break;
        }
    }
    out
}

/// Whether two lyrics are the same words, in order. Traditional and Simplified Chinese versions
/// of one song still share most characters; different songs share few in sequence.
pub fn same_lyrics(a: &Parsed, b: &Parsed) -> bool {
    let (a, b) = (lyric_tokens(a), lyric_tokens(b));
    let shorter = a.len().min(b.len());
    if shorter < MIN_COMPARE_TOKENS {
        return false;
    }
    let mut row = vec![0u32; b.len() + 1];
    for x in &a {
        let mut diagonal = 0;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if x == y { diagonal + 1 } else { above.max(row[j]) };
            diagonal = above;
        }
    }
    row[b.len()] as f64 / shorter as f64 >= SAME_LYRICS_RATIO
}

/// Untimed lyrics being shown, which a timed version of the same words may replace.
struct Untimed {
    parsed: Parsed,
    source: String,
    pinned: bool,
}

impl Untimed {
    fn resolved(self) -> Resolved {
        ok(self.parsed, &self.source)
    }
}

/// `keep_going` is checked before each provider call: once false (the song is no longer current)
/// the lookup stops and writes nothing.
pub fn resolve(db: &Mutex<Db>, song: &SongRow, audio: Option<&Path>, providers: &[&dyn Provider], now: i64, keep_going: &dyn Fn() -> bool) -> Resolved {
    let cached = lock(db).lyric(&song.id).ok().flatten();
    let mut untimed: Option<Untimed> = None;

    // 1. manual choice; an untimed online choice may still gain timing
    if let Some(row) = cached.as_ref().filter(|r| r.pinned && r.status == "ok") {
        let p = parsed_of(row);
        if !p.is_empty() {
            let source = row.source.as_deref().unwrap_or("manual");
            if p.kind() != "plain" || source == "manual" {
                return ok(p, source);
            }
            untimed = Some(Untimed { parsed: p, source: source.to_string(), pinned: true });
        }
    }

    // 2-4. local sources (never cached; no db lock held)
    let is_cue = song.start_ms > 0 || song.end_ms.is_some();
    if let (false, Some(audio), true) = (is_cue, audio, untimed.is_none()) {
        let mut embedded_plain = None;
        if let Some(text) = crate::tags::read_lyrics(audio) {
            let p = parse_lrc(&text);
            if !p.is_empty() {
                if p.kind() != "plain" {
                    return ok(p, "embedded");
                }
                embedded_plain = Some(p);
            }
        }
        if let Some(text) = super::local::sibling_lrc(audio) {
            let p = parse_lrc(&text);
            if !p.is_empty() {
                return ok(p, "file");
            }
        }
        untimed = embedded_plain.map(|parsed| Untimed { parsed, source: "embedded".into(), pinned: false });
    }

    // 5-6. cache
    let key = none_key(song, providers);
    if let Some(row) = cached.as_ref().filter(|r| !r.pinned || r.status != "ok") {
        if row.status == "ok" {
            let p = parsed_of(row);
            if !p.is_empty() {
                let source = row.source.as_deref().unwrap_or("manual");
                if p.kind() != "plain" {
                    return ok(p, source);
                }
                if untimed.is_none() {
                    untimed = Some(Untimed { parsed: p, source: source.to_string(), pinned: false });
                }
            }
        } else if untimed.is_none() && row.status == "none" && row.ext_id.as_deref() == Some(key.as_str())
            && now < row.fetched.saturating_add(NONE_TTL_SECS) {
            return status_only("none");
        }
    }
    if untimed.is_some() && lock(db).lyric_upgrade_checked(&song.id, &key, now - NONE_TTL_SECS).unwrap_or(false) {
        return untimed.map(Untimed::resolved).unwrap_or_else(|| status_only("none"));
    }

    // 7. providers: the first timed lyric wins; untimed text is only a fallback
    if providers.is_empty() {
        return untimed.map_or_else(|| status_only("none"), Untimed::resolved);
    }
    let q = Query { title: song.title.clone(), artist: song.artist.clone(), album: song.album.clone(), duration: song.duration };
    let mut failed = false;
    let mut plain_hit: Option<(&'static str, Raw, Parsed)> = None;
    for p in providers {
        if !keep_going() {
            return untimed.map_or_else(|| status_only("retry"), Untimed::resolved);
        }
        match p.best(&q) {
            Ok(Some(raw)) => {
                let parsed = parse_texts(raw.yrc.as_deref(), raw.lrc.as_deref(), raw.plain.as_deref(), raw.trans.as_deref());
                if parsed.is_empty() {
                    continue;
                }
                if parsed.kind() == "plain" {
                    if plain_hit.is_none() {
                        plain_hit = Some((p.name(), raw, parsed));
                    }
                    continue;
                }
                // a timed lyric only replaces existing text when it is the same song's words
                if untimed.as_ref().is_some_and(|u| !same_lyrics(&u.parsed, &parsed)) {
                    continue;
                }
                let row = row_of(p.name(), raw, now);
                if untimed.as_ref().is_some_and(|u| u.pinned) {
                    let _ = lock(db).upgrade_pinned_lyric(&song.id, &row);
                } else {
                    let _ = lock(db).put_fetched_lyric(&song.id, &row);
                }
                return ok(parsed, p.name());
            }
            Ok(None) => {}
            Err(_) => failed = true,
        }
    }
    if let Some(u) = untimed {
        if !failed {
            let _ = lock(db).mark_lyric_upgrade_checked(&song.id, &key, now);
        }
        return u.resolved();
    }
    if let Some((name, raw, parsed)) = plain_hit {
        let row = row_of(name, raw, now);
        let _ = lock(db).put_fetched_lyric(&song.id, &row);
        if !failed {
            let _ = lock(db).mark_lyric_upgrade_checked(&song.id, &key, now);
        }
        return ok(parsed, name);
    }
    if failed {
        return status_only("retry");
    }
    let row = LyricRow { status: "none".into(), ext_id: Some(key), fetched: now, ..Default::default() };
    let _ = lock(db).put_fetched_lyric(&song.id, &row);
    status_only("none")
}

fn row_of(name: &str, raw: Raw, now: i64) -> LyricRow {
    LyricRow {
        status: "ok".into(), source: Some(name.to_string()), ext_id: Some(raw.ext_id),
        lrc: raw.lrc, yrc: raw.yrc, trans: raw.trans, plain: raw.plain, pinned: false, fetched: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::online::{Candidate, NetError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake { name: &'static str, calls: AtomicUsize, out: Result<Option<&'static str>, ()> }
    impl Fake {
        fn new(name: &'static str, out: Result<Option<&'static str>, ()>) -> Fake {
            Fake { name, calls: AtomicUsize::new(0), out }
        }
        fn n(&self) -> usize { self.calls.load(Ordering::SeqCst) }
    }
    impl Provider for Fake {
        fn name(&self) -> &'static str { self.name }
        fn best(&self, _: &Query) -> Result<Option<Raw>, NetError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.out {
                Err(()) => Err(NetError),
                Ok(None) => Ok(None),
                Ok(Some(t)) => Ok(Some(Raw { source: self.name.into(), ext_id: "7".into(), lrc: Some(t.into()), ..Default::default() })),
            }
        }
        fn search(&self, _: &Query) -> Result<Vec<Candidate>, NetError> { Ok(vec![]) }
        fn fetch(&self, _: &str) -> Result<Option<Raw>, NetError> { Ok(None) }
    }

    fn timed() -> &'static str { "[00:01.00]hello\n[00:02.00]world" }

    /// Most tests do not care about song changes.
    fn resolve(db: &Mutex<Db>, song: &SongRow, audio: Option<&Path>, providers: &[&dyn Provider], now: i64) -> Resolved {
        super::resolve(db, song, audio, providers, now, &|| true)
    }

    fn song() -> SongRow {
        SongRow { id: "s1".into(), title: "晴天".into(), artist: "周杰伦".into(), album: "B".into(), duration: 200.0,
                  root_id: 1, file_id: 1, rel: "a.mp3".into(), ext: "mp3".into(), codec: "mp3".into(), bitrate: 0, size: 0,
                  cover: 0, track: 1, start_ms: 0, end_ms: None, mtime: 0, added: None }
    }
    fn mem() -> Mutex<Db> { Mutex::new(Db::open_in_memory().unwrap()) }

    /// An mp3 whose ID3v2.3 tag carries a USLT frame with `text`.
    fn mp3_with(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
        let mut body = vec![0u8]; // latin1/utf8-ish encoding byte 0
        body.extend_from_slice(b"eng\0");
        body.extend_from_slice(text.as_bytes());
        let mut frame = b"USLT".to_vec();
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&body);
        let n = frame.len();
        let mut f = b"ID3\x03\x00\x00".to_vec();
        f.extend_from_slice(&[(n >> 21) as u8 & 0x7f, (n >> 14) as u8 & 0x7f, (n >> 7) as u8 & 0x7f, n as u8 & 0x7f]);
        f.extend_from_slice(&frame);
        let p = dir.join(name);
        std::fs::write(&p, f).unwrap();
        p
    }

    #[test] fn embedded_beats_online() {
        let d = tempfile::tempdir().unwrap();
        let a = mp3_with(d.path(), "a.mp3", timed());
        let f = Fake::new("lrclib", Ok(Some(timed())));
        let r = resolve(&mem(), &song(), Some(&a), &[&f], 100);
        assert_eq!((r.status, r.source.as_deref()), ("ok", Some("embedded")));
        assert_eq!(f.n(), 0);
    }
    #[test] fn timed_lrc_beats_plain_embedded() {
        let d = tempfile::tempdir().unwrap();
        let a = mp3_with(d.path(), "a.mp3", "just plain\ntext");
        std::fs::write(d.path().join("a.lrc"), timed()).unwrap();
        let f = Fake::new("lrclib", Ok(None));
        let r = resolve(&mem(), &song(), Some(&a), &[&f], 100);
        assert_eq!(r.source.as_deref(), Some("file"));
        assert_eq!(r.parsed.kind(), "line");
    }
    #[test] fn plain_embedded_used_when_nothing_better() {
        let d = tempfile::tempdir().unwrap();
        let a = mp3_with(d.path(), "a.mp3", "just plain\ntext");
        let f = Fake::new("lrclib", Ok(Some(timed())));
        let r = resolve(&mem(), &song(), Some(&a), &[&f], 100);
        assert_eq!((r.source.as_deref(), r.parsed.kind()), (Some("embedded"), "plain"));
        assert_eq!(f.n(), 1); // looked for timing, but those are other words
    }
    const WORDS: &str = "morning light on the water\nwe walk where the road runs thin\ncounting the lamps as they fade";
    const TIMED_WORDS: &str = "[00:01.00]morning light on the water\n[00:05.00]we walk where the road runs thin\n[00:09.00]counting the lamps as they fade";
    const OTHER_WORDS: &str = "[00:01.00]every window holds a song\n[00:05.00]slow rivers bring us home\n[00:09.00]tomorrow answers back again";
    fn parsed(text: &str) -> Parsed { parse_lrc(text) }

    #[test] fn same_words_are_recognized_across_scripts() {
        assert!(same_lyrics(&parsed(WORDS), &parsed(TIMED_WORDS)));
        assert!(!same_lyrics(&parsed(WORDS), &parsed(OTHER_WORDS)));
        let traditional = "將你穿過的球鞋留給我\n走過了泥濘只有我才懂\n和你不能笑著走到盡頭";
        let simplified = "[00:01.00]将你穿过的球鞋留给我\n[00:05.00]走过了泥泞只有我才懂\n[00:09.00]和你不能笑着走到尽头";
        assert!(same_lyrics(&parsed(traditional), &parsed(simplified)));
        assert!(!same_lyrics(&parsed("短"), &parsed("[00:01.00]短")));
    }
    #[test] fn embedded_plain_gains_timing_from_the_same_lyric() {
        let d = tempfile::tempdir().unwrap();
        let a = mp3_with(d.path(), "a.mp3", WORDS);
        let db = mem();
        let f = Fake::new("netease", Ok(Some(TIMED_WORDS)));
        let r = resolve(&db, &song(), Some(&a), &[&f], 100);
        assert_eq!((r.source.as_deref(), r.parsed.kind()), (Some("netease"), "line"));
        let again = resolve(&db, &song(), Some(&a), &[&f], 200);
        assert_eq!(again.parsed.kind(), "line");
        assert_eq!(f.n(), 1);
    }
    #[test] fn plain_from_one_site_does_not_stop_timed_from_the_next() {
        let db = mem();
        let a = Fake::new("lrclib", Ok(Some(WORDS)));
        let b = Fake::new("netease", Ok(Some(TIMED_WORDS)));
        let r = resolve(&db, &song(), None, &[&a, &b], 1);
        assert_eq!((r.source.as_deref(), r.parsed.kind()), (Some("netease"), "line"));
    }
    #[test] fn plain_is_kept_when_no_site_has_timing() {
        let db = mem();
        let a = Fake::new("lrclib", Ok(Some(WORDS)));
        let b = Fake::new("netease", Ok(None));
        let r = resolve(&db, &song(), None, &[&a, &b], 1);
        assert_eq!((r.source.as_deref(), r.parsed.kind()), (Some("lrclib"), "plain"));
        // no timed version: not asked again for a week
        resolve(&db, &song(), None, &[&a, &b], 2);
        assert_eq!((a.n(), b.n()), (1, 1));
        resolve(&db, &song(), None, &[&a, &b], 2 + NONE_TTL_SECS);
        assert_eq!((a.n(), b.n()), (2, 2));
    }
    fn pin_plain(db: &Mutex<Db>, source: &str) {
        let row = LyricRow { status: "ok".into(), source: Some(source.into()), ext_id: Some("3".into()), plain: Some(WORDS.into()),
                             pinned: true, fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
    }
    #[test] fn a_plain_online_choice_gains_timing_and_stays_chosen() {
        let db = mem();
        pin_plain(&db, "lrclib");
        let f = Fake::new("netease", Ok(Some(TIMED_WORDS)));
        let r = resolve(&db, &song(), None, &[&f], 5);
        assert_eq!(r.parsed.kind(), "line");
        let row = db.lock().unwrap().lyric("s1").unwrap().unwrap();
        assert!(row.pinned);
        assert_eq!(row.source.as_deref(), Some("netease"));
    }
    #[test] fn a_plain_choice_is_not_replaced_by_other_words() {
        let db = mem();
        pin_plain(&db, "lrclib");
        let f = Fake::new("netease", Ok(Some(OTHER_WORDS)));
        let r = resolve(&db, &song(), None, &[&f], 5);
        assert_eq!((r.source.as_deref(), r.parsed.kind()), (Some("lrclib"), "plain"));
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap().unwrap().plain.as_deref(), Some(WORDS));
    }
    #[test] fn a_lyric_file_the_user_chose_is_never_looked_up() {
        let db = mem();
        pin_plain(&db, "manual");
        let f = Fake::new("netease", Ok(Some(TIMED_WORDS)));
        assert_eq!(resolve(&db, &song(), None, &[&f], 5).parsed.kind(), "plain");
        assert_eq!(f.n(), 0);
    }
    #[test] fn a_failed_upgrade_still_shows_the_text_and_tries_again() {
        let db = mem();
        pin_plain(&db, "lrclib");
        let bad = Fake::new("netease", Err(()));
        assert_eq!(resolve(&db, &song(), None, &[&bad], 5).status, "ok");
        assert_eq!(resolve(&db, &song(), None, &[&bad], 6).status, "ok");
        assert_eq!(bad.n(), 2);
    }
    #[test] fn online_hit_is_cached() {
        let db = mem();
        let f = Fake::new("lrclib", Ok(Some(timed())));
        let r = resolve(&db, &song(), None, &[&f], 100);
        assert_eq!((r.status, r.source.as_deref()), ("ok", Some("lrclib")));
        let row = db.lock().unwrap().lyric("s1").unwrap().unwrap();
        assert_eq!((row.status.as_str(), row.ext_id.as_deref(), row.pinned, row.fetched), ("ok", Some("7"), false, 100));
        let r2 = resolve(&db, &song(), None, &[&f], 200);
        assert_eq!((r2.status, r2.source.as_deref()), ("ok", Some("lrclib")));
        assert_eq!(f.n(), 1);
    }
    #[test] fn none_is_cached_for_seven_days() {
        let db = mem();
        let f = Fake::new("lrclib", Ok(None));
        assert_eq!(resolve(&db, &song(), None, &[&f], 1000).status, "none");
        assert_eq!(f.n(), 1);
        assert_eq!(resolve(&db, &song(), None, &[&f], 1000 + NONE_TTL_SECS - 1).status, "none");
        assert_eq!(f.n(), 1);
        assert_eq!(resolve(&db, &song(), None, &[&f], 1000 + NONE_TTL_SECS + 1).status, "none");
        assert_eq!(f.n(), 2);
    }
    #[test] fn network_error_is_not_cached() {
        let db = mem();
        let f = Fake::new("lrclib", Err(()));
        assert_eq!(resolve(&db, &song(), None, &[&f], 1).status, "retry");
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), None);
        assert_eq!(resolve(&db, &song(), None, &[&f], 2).status, "retry");
        assert_eq!(f.n(), 2);
    }
    #[test] fn later_provider_used_when_first_fails() {
        let db = mem();
        let a = Fake::new("lrclib", Err(()));
        let b = Fake::new("netease", Ok(Some(timed())));
        let r = resolve(&db, &song(), None, &[&a, &b], 1);
        assert_eq!((r.status, r.source.as_deref()), ("ok", Some("netease")));
    }
    #[test] fn empty_answer_counts_as_none() {
        let db = mem();
        let a = Fake::new("lrclib", Ok(Some("  ")));
        assert_eq!(resolve(&db, &song(), None, &[&a], 1).status, "none");
    }
    #[test] fn failed_retry_keeps_old_none_row() {
        let db = mem();
        let a = Fake::new("lrclib", Ok(None));
        resolve(&db, &song(), None, &[&a], 1000);
        let bad = Fake::new("lrclib", Err(()));
        let later = 1000 + NONE_TTL_SECS + 5;
        assert_eq!(resolve(&db, &song(), None, &[&bad], later).status, "retry");
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap().unwrap().fetched, 1000);
    }
    #[test] fn no_providers_writes_nothing() {
        let db = mem();
        assert_eq!(resolve(&db, &song(), None, &[], 1).status, "none");
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), None);
    }
    fn pin(db: &Mutex<Db>) {
        let row = LyricRow { status: "ok".into(), source: Some("manual".into()), lrc: Some("[00:05.00]mine".into()),
                             pinned: true, fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
    }
    fn ok_row(db: &Mutex<Db>) {
        let row = LyricRow { status: "ok".into(), source: Some("netease".into()), lrc: Some(timed().into()), fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
    }
    #[test] fn pinned_survives_provider_hit_and_none() {
        // empty-parsing pinned row falls through to providers but must not be replaced
        let db = mem();
        let row = LyricRow { status: "ok".into(), source: Some("manual".into()), lrc: Some("".into()), pinned: true, fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
        resolve(&db, &song(), None, &[&Fake::new("lrclib", Ok(Some(timed())))], 5);
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), Some(row.clone()));
        resolve(&db, &song(), None, &[&Fake::new("lrclib", Ok(None))], 5);
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), Some(row));
    }
    struct Racer<'a> { db: &'a Mutex<Db> }
    impl Provider for Racer<'_> {
        fn name(&self) -> &'static str { "lrclib" }
        fn best(&self, _: &Query) -> Result<Option<Raw>, NetError> {
            pin(self.db); // db lock must be free here
            Ok(Some(Raw { ext_id: "7".into(), lrc: Some(timed().into()), ..Default::default() }))
        }
        fn search(&self, _: &Query) -> Result<Vec<Candidate>, NetError> { Ok(vec![]) }
        fn fetch(&self, _: &str) -> Result<Option<Raw>, NetError> { Ok(None) }
    }
    #[test] fn pin_made_during_lookup_survives() {
        let db = mem();
        resolve(&db, &song(), None, &[&Racer { db: &db }], 5);
        let row = db.lock().unwrap().lyric("s1").unwrap().unwrap();
        assert!(row.pinned);
        assert_eq!(row.lrc.as_deref(), Some("[00:05.00]mine"));
    }
    #[test] fn ok_row_survives_none_and_failure() {
        let db = mem();
        // an ok row whose text parses empty: providers all say none
        let row = LyricRow { status: "ok".into(), source: Some("netease".into()), lrc: Some("".into()), fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
        assert_eq!(resolve(&db, &song(), None, &[&Fake::new("lrclib", Ok(None))], 5).status, "none");
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), Some(row));
        // a valid ok row survives a failed lookup (never reaches providers)
        let db = mem();
        ok_row(&db);
        let bad = Fake::new("lrclib", Err(()));
        assert_eq!(resolve(&db, &song(), None, &[&bad], 5).status, "ok");
        assert_eq!(bad.n(), 0);
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap().unwrap().fetched, 1);
    }
    #[test] fn pinned_wins_over_embedded() {
        let d = tempfile::tempdir().unwrap();
        let a = mp3_with(d.path(), "a.mp3", timed());
        let db = mem();
        let row = LyricRow { status: "ok".into(), source: Some("manual".into()), lrc: Some("[00:05.00]mine".into()),
                             pinned: true, fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
        let f = Fake::new("lrclib", Ok(Some(timed())));
        let r = resolve(&db, &song(), Some(&a), &[&f], 100);
        assert_eq!(r.source.as_deref(), Some("manual"));
        assert_eq!(r.parsed.lines[0].text, "mine");
        assert_eq!(f.n(), 0);
    }
    #[test] fn cue_track_skips_local_sources() {
        let d = tempfile::tempdir().unwrap();
        let a = mp3_with(d.path(), "a.mp3", "x");
        std::fs::write(d.path().join("a.lrc"), timed()).unwrap();
        let mut s = song();
        s.end_ms = Some(5000);
        let f = Fake::new("lrclib", Ok(Some("[00:09.00]online")));
        let r = resolve(&mem(), &s, Some(&a), &[&f], 1);
        assert_eq!(r.source.as_deref(), Some("lrclib"));
        assert_eq!(f.n(), 1);
    }
    #[test] fn offline_folder_uses_cache() {
        let db = mem();
        let row = LyricRow { status: "ok".into(), source: Some("netease".into()), lrc: Some(timed().into()), fetched: 1, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &row).unwrap();
        let f = Fake::new("lrclib", Ok(None));
        let r = resolve(&db, &song(), None, &[&f], 5);
        assert_eq!((r.status, r.source.as_deref()), ("ok", Some("netease")));
        assert_eq!(f.n(), 0);
    }
    #[test] fn song_change_before_first_provider_asks_nobody() {
        let db = mem();
        let f = Fake::new("lrclib", Ok(Some(timed())));
        let r = super::resolve(&db, &song(), None, &[&f], 1, &|| false);
        assert_eq!(r.status, "retry");
        assert_eq!(f.n(), 0);
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), None);
    }
    #[test] fn song_change_between_providers_stops_and_writes_nothing() {
        let db = mem();
        let a = Fake::new("lrclib", Ok(None));
        let b = Fake::new("netease", Ok(Some(timed())));
        let r = super::resolve(&db, &song(), None, &[&a, &b], 1, &|| a.n() == 0);
        assert_eq!(r.status, "retry");
        assert_eq!((a.n(), b.n()), (1, 0));
        assert_eq!(db.lock().unwrap().lyric("s1").unwrap(), None);
    }
    #[test] fn none_row_rechecks_when_netease_is_enabled() {
        let db = mem();
        let a = Fake::new("lrclib", Ok(None));
        assert_eq!(resolve(&db, &song(), None, &[&a], 1000).status, "none");
        let b = Fake::new("netease", Ok(Some(timed())));
        let r = resolve(&db, &song(), None, &[&a, &b], 1001);
        assert_eq!((r.status, r.source.as_deref()), ("ok", Some("netease")));
        assert_eq!(b.n(), 1);
    }
    #[test] fn none_row_rechecks_when_tags_change() {
        for change in [
            (|s: &mut SongRow| s.title = "雨天".into()) as fn(&mut SongRow),
            |s| s.artist = "蔡依林".into(),
            |s| s.album = "C".into(),
            |s| s.duration = 201.0,
        ] {
            let db = mem();
            let f = Fake::new("lrclib", Ok(None));
            resolve(&db, &song(), None, &[&f], 1000);
            let mut s = song();
            change(&mut s);
            resolve(&db, &s, None, &[&f], 1001);
            assert_eq!(f.n(), 2);
        }
    }
    #[test] fn none_row_stays_cached_under_same_conditions() {
        let db = mem();
        let f = Fake::new("lrclib", Ok(None));
        resolve(&db, &song(), None, &[&f], 1000);
        let mut s = song();
        s.title = " 晴天 ".into();
        s.artist = "周杰伦 ".into();
        s.duration = 200.3;
        assert_eq!(resolve(&db, &s, None, &[&f], 1001).status, "none");
        assert_eq!(f.n(), 1);
    }
    #[test] fn none_row_ttl_boundary_is_exact() {
        let db = mem();
        let f = Fake::new("lrclib", Ok(None));
        resolve(&db, &song(), None, &[&f], 1000);
        resolve(&db, &song(), None, &[&f], 1000 + NONE_TTL_SECS - 1);
        assert_eq!(f.n(), 1);
        resolve(&db, &song(), None, &[&f], 1000 + NONE_TTL_SECS);
        assert_eq!(f.n(), 2);
    }
    #[test] fn none_row_without_fingerprint_is_rechecked() {
        let db = mem();
        let old = LyricRow { status: "none".into(), fetched: 1000, ..Default::default() };
        db.lock().unwrap().put_lyric("s1", &old).unwrap();
        let f = Fake::new("lrclib", Ok(None));
        resolve(&db, &song(), None, &[&f], 1001);
        assert_eq!(f.n(), 1);
        assert!(db.lock().unwrap().lyric("s1").unwrap().unwrap().ext_id.is_some());
    }
    #[test] fn parsed_of_prefers_yrc_and_merges_trans() {
        let row = LyricRow { status: "ok".into(), yrc: Some("[1000,1000](1000,500,0)你(1500,500,0)好".into()),
            lrc: Some(timed().into()), trans: Some("[00:01.00]hi".into()), ..Default::default() };
        let p = parsed_of(&row);
        assert_eq!(p.kind(), "word");
        assert_eq!(p.lines[0].trans.as_deref(), Some("hi"));
    }
}
