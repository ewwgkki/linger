//! Source order, cache and retry rules for lyrics.
use super::online::{Provider, Query, Raw};
use super::{merge_translation, parse_lrc, parse_yrc, Parsed};
use crate::db::{Db, LyricRow, SongRow};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub const NONE_TTL_SECS: i64 = 7 * 24 * 3600;

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

pub fn resolve(db: &Mutex<Db>, song: &SongRow, audio: Option<&Path>, providers: &[&dyn Provider], now: i64) -> Resolved {
    let cached = lock(db).lyric(&song.id).ok().flatten();

    // 1. manual choice
    if let Some(row) = cached.as_ref().filter(|r| r.pinned && r.status == "ok") {
        let p = parsed_of(row);
        if !p.is_empty() {
            return ok(p, row.source.as_deref().unwrap_or("manual"));
        }
    }

    // 2-4. local sources (never cached; no db lock held)
    let is_cue = song.start_ms > 0 || song.end_ms.is_some();
    let mut embedded_plain = None;
    if let (false, Some(audio)) = (is_cue, audio) {
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
        if let Some(p) = embedded_plain {
            return ok(p, "embedded");
        }
    }

    // 5-6. cache
    if let Some(row) = &cached {
        if row.status == "ok" {
            let p = parsed_of(row);
            if !p.is_empty() {
                return ok(p, row.source.as_deref().unwrap_or("manual"));
            }
        } else if row.status == "none" && now < row.fetched.saturating_add(NONE_TTL_SECS) {
            return status_only("none");
        }
    }

    // 7. providers
    if providers.is_empty() {
        return status_only("none");
    }
    let q = Query { title: song.title.clone(), artist: song.artist.clone(), album: song.album.clone(), duration: song.duration };
    let mut failed = false;
    for p in providers {
        match p.best(&q) {
            Ok(Some(raw)) => {
                let parsed = parse_texts(raw.yrc.as_deref(), raw.lrc.as_deref(), raw.plain.as_deref(), raw.trans.as_deref());
                if parsed.is_empty() {
                    continue;
                }
                let row = row_of(p.name(), raw, now);
                let _ = lock(db).put_lyric(&song.id, &row);
                return ok(parsed, p.name());
            }
            Ok(None) => {}
            Err(_) => failed = true,
        }
    }
    if failed {
        return status_only("retry");
    }
    let row = LyricRow { status: "none".into(), fetched: now, ..Default::default() };
    let _ = lock(db).put_lyric(&song.id, &row);
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
        assert_eq!(f.n(), 0);
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
    #[test] fn parsed_of_prefers_yrc_and_merges_trans() {
        let row = LyricRow { status: "ok".into(), yrc: Some("[1000,1000](1000,500,0)你(1500,500,0)好".into()),
            lrc: Some(timed().into()), trans: Some("[00:01.00]hi".into()), ..Default::default() };
        let p = parsed_of(&row);
        assert_eq!(p.kind(), "word");
        assert_eq!(p.lines[0].trans.as_deref(), Some("hi"));
    }
}
