//! CUE sheet parsing and whole-album image splitting.

use crate::derive::folder_artist_album;
use crate::text::decode_legacy;
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CueSheet {
    pub title: Option<String>,
    pub performer: Option<String>,
    pub files: Vec<CueFile>,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CueFile {
    pub name: String,
    pub tracks: Vec<CueTrack>,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CueTrack {
    pub number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub start_ms: u64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CueSong {
    pub file_name: String,
    pub number: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub start_ms: u64,
    /// None = until end of file
    pub end_ms: Option<u64>,
}

fn decode_text(bytes: &[u8]) -> String {
    if let Some(r) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(r).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_legacy(bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => decode_legacy(bytes),
    }
}

/// Split `"quoted value" rest` or `unquoted rest` into (value, rest).
fn take_value(s: &str) -> (String, &str) {
    let s = s.trim();
    if let Some(r) = s.strip_prefix('"') {
        match r.find('"') {
            Some(i) => (r[..i].to_string(), r[i + 1..].trim()),
            None => (r.to_string(), ""),
        }
    } else {
        match s.find(char::is_whitespace) {
            Some(i) => (s[..i].to_string(), s[i..].trim()),
            None => (s.to_string(), ""),
        }
    }
}

fn parse_time(s: &str) -> Option<u64> {
    let p: Vec<&str> = s.trim().split(':').collect();
    if p.len() != 3 {
        return None;
    }
    let (m, sec, f): (u64, u64, u64) = (p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?);
    Some((m * 60 + sec) * 1000 + f * 1000 / 75)
}

pub fn parse_cue(bytes: &[u8]) -> Option<CueSheet> {
    let text = decode_text(bytes);
    let mut sheet = CueSheet::default();
    let mut skip_file = false; // current FILE is not audio
    let mut cur: Option<CueTrack> = None; // track being built (start_ms valid only if has_index)
    let mut has_index = false;

    fn flush(sheet: &mut CueSheet, cur: &mut Option<CueTrack>, has_index: &mut bool, skip: bool) {
        if let Some(t) = cur.take() {
            if *has_index && !skip {
                if let Some(f) = sheet.files.last_mut() {
                    f.tracks.push(t);
                }
            }
        }
        *has_index = false;
    }

    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        let (cmd, rest) = match line.find(char::is_whitespace) {
            Some(i) => (&line[..i], line[i..].trim()),
            None => (line, ""),
        };
        match cmd.to_ascii_uppercase().as_str() {
            "FILE" => {
                flush(&mut sheet, &mut cur, &mut has_index, skip_file);
                let (name, ty) = take_value(rest);
                let ty = ty.to_ascii_uppercase();
                skip_file = ty == "BINARY" || ty == "MOTOROLA";
                if !skip_file {
                    sheet.files.push(CueFile { name, tracks: vec![] });
                }
            }
            "TRACK" => {
                flush(&mut sheet, &mut cur, &mut has_index, skip_file);
                let (n, _) = take_value(rest);
                cur = Some(CueTrack { number: n.parse().unwrap_or(0), ..Default::default() });
            }
            "TITLE" | "PERFORMER" => {
                let (v, _) = take_value(rest);
                let v = Some(v);
                let is_title = cmd.eq_ignore_ascii_case("TITLE");
                match cur.as_mut() {
                    Some(t) => {
                        if is_title { t.title = v } else { t.performer = v }
                    }
                    None => {
                        if is_title { sheet.title = v } else { sheet.performer = v }
                    }
                }
            }
            "INDEX" => {
                let (n, t) = take_value(rest);
                if n == "01" {
                    if let (Some(tr), Some(ms)) = (cur.as_mut(), parse_time(t)) {
                        tr.start_ms = ms;
                        has_index = true;
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut sheet, &mut cur, &mut has_index, skip_file);
    sheet.files.retain(|f| !f.tracks.is_empty());
    if sheet.files.is_empty() { None } else { Some(sheet) }
}

fn tracklist_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(\d{1,2})[\s.、\-_]+(.+?)\s*$").unwrap())
}

/// Longest run of lines numbered 1,2,3..N (N >= 2); empty otherwise.
pub fn parse_tracklist_txt(bytes: &[u8]) -> Vec<(u32, String)> {
    let text = decode_text(bytes);
    let mut best: Vec<(u32, String)> = vec![];
    let mut run: Vec<(u32, String)> = vec![];
    for line in text.lines() {
        let Some(c) = tracklist_re().captures(line) else { continue };
        let n: u32 = c[1].parse().unwrap();
        let name = c[2].trim().to_string();
        if n == 1 {
            if run.len() > best.len() {
                best = std::mem::take(&mut run);
            }
            run = vec![(1, name)];
        } else if !run.is_empty() && n == run.len() as u32 + 1 {
            run.push((n, name));
        }
    }
    if run.len() > best.len() {
        best = run;
    }
    if best.len() >= 2 { best } else { vec![] }
}

pub fn is_placeholder(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let s = s.trim();
    s.is_empty()
        || RE
            .get_or_init(|| Regex::new(r"(?i)^(track\s*\d+|audio\s*track.*|未知.*|unknown.*)$").unwrap())
            .is_match(s)
}

fn real(o: &Option<String>) -> Option<&str> {
    o.as_deref().filter(|s| !is_placeholder(s))
}

pub fn resolve(sheet: &CueSheet, folder: &str, tracklist: &[(u32, String)]) -> Vec<CueSong> {
    let (f_artist, f_album) = folder_artist_album(folder);
    let album = real(&sheet.title)
        .map(str::to_string)
        .or_else(|| (!f_album.is_empty()).then(|| f_album.clone()))
        .unwrap_or_else(|| folder.to_string());
    let mut out = vec![];
    for file in &sheet.files {
        // A list may be shorter than the CUE (unlisted hidden track): its names go to
        // tracks 1..N by number; a list longer than the CUE belongs to something else.
        let use_list = !tracklist.is_empty() && tracklist.len() <= file.tracks.len();
        for (i, t) in file.tracks.iter().enumerate() {
            let title = real(&t.title)
                .map(str::to_string)
                .or_else(|| {
                    if use_list {
                        tracklist.iter().find(|(n, _)| *n == t.number).map(|(_, s)| s.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| format!("Track {:02}", t.number));
            let artist = real(&t.performer)
                .or_else(|| real(&sheet.performer))
                .map(str::to_string)
                .unwrap_or_else(|| f_artist.clone());
            out.push(CueSong {
                file_name: file.name.clone(),
                number: t.number,
                title,
                artist,
                album: album.clone(),
                start_ms: t.start_ms,
                end_ms: file.tracks.get(i + 1).map(|n| n.start_ms),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gbk(s: &str) -> Vec<u8> {
        encoding_rs::GBK.encode(s).0.into_owned()
    }

    fn sample_cue() -> String {
        let mut s = String::from(
            "REM DISCID 00000000\r\nREM COMMENT \"x\"\r\nPERFORMER \"未知艺术家\"\r\nTITLE \"未知标题\"\r\nFILE \"未知艺术家 - 未知标题.wav\" WAVE\r\n",
        );
        let starts = ["00:00:00", "05:33:37", "09:33:00", "14:56:00", "18:35:00", "22:43:00", "26:00:00", "30:00:00", "34:00:00", "38:00:00"];
        for (i, st) in starts.iter().enumerate() {
            s += &format!(
                "  TRACK {n:02} AUDIO\r\n    TITLE \"Track{n:02}\"\r\n    PERFORMER \"未知艺术家\"\r\n    INDEX 01 {st}\r\n",
                n = i + 1
            );
        }
        s
    }

    const NAMES: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
    fn list_txt(n: usize) -> String {
        let mut s = String::from("描述 1 段落\r\n2001 年发行\r\n\r\n专辑曲目:\r\n\r\n");
        for i in 0..n {
            s += &format!("{:02} {}\r\n", i + 1, NAMES[i]);
        }
        s
    }

    #[test]
    fn gbk_cue_with_placeholder_titles_uses_tracklist() {
        let sheet = parse_cue(&gbk(&sample_cue())).unwrap();
        let list = parse_tracklist_txt(&gbk(&list_txt(10)));
        assert_eq!(list.len(), 10);
        let songs = resolve(&sheet, "示例歌手《晴天集》首版 典藏[整轨WAV]", &list);
        assert_eq!(songs.len(), 10);
        assert_eq!(songs[0].file_name, "未知艺术家 - 未知标题.wav");
        assert_eq!((songs[1].title.as_str(), songs[1].artist.as_str(), songs[1].album.as_str()), ("乙", "示例歌手", "晴天集"));
        assert_eq!(songs[1].start_ms, 333_493);
        assert_eq!(songs[0].end_ms, Some(333_493));
        assert_eq!(songs[9].end_ms, None);
    }

    #[test]
    fn real_titles_win_over_tracklist() {
        let cue = "PERFORMER \"周杰伦\"\nTITLE \"叶惠美\"\nFILE \"a.wav\" WAVE\n TRACK 01 AUDIO\n  TITLE \"晴天\"\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  TITLE \"Track02\"\n  INDEX 01 04:00:00\n";
        let sheet = parse_cue(cue.as_bytes()).unwrap();
        let list = vec![(1, "别的".to_string()), (2, "次".to_string())];
        let s = resolve(&sheet, "x", &list);
        assert_eq!((s[0].title.as_str(), s[0].artist.as_str(), s[0].album.as_str()), ("晴天", "周杰伦", "叶惠美"));
        assert_eq!(s[1].title, "次");
    }

    #[test]
    fn shorter_tracklist_names_leading_tracks() {
        // e.g. a hidden 10th track the booklet does not list
        let sheet = parse_cue(&gbk(&sample_cue())).unwrap();
        let list = parse_tracklist_txt(&gbk(&list_txt(9)));
        assert_eq!(list.len(), 9);
        let songs = resolve(&sheet, "示例歌手《晴天集》x", &list);
        assert_eq!(songs[0].title, "甲");
        assert_eq!(songs[8].title, "壬");
        assert_eq!(songs[9].title, "Track 10");
    }

    #[test]
    fn longer_tracklist_is_ignored() {
        let cue = "FILE \"a.wav\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  INDEX 01 01:00:00\n";
        let sheet = parse_cue(cue.as_bytes()).unwrap();
        let list = parse_tracklist_txt(&gbk(&list_txt(3)));
        let s = resolve(&sheet, "f", &list);
        assert_eq!((s[0].title.as_str(), s[1].title.as_str()), ("Track 01", "Track 02"));
    }

    #[test]
    fn utf8_bom_cue() {
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice("TITLE \"专辑\"\nFILE \"a.wav\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n".as_bytes());
        let s = parse_cue(&b).unwrap();
        assert_eq!(s.title.as_deref(), Some("专辑"));
        assert_eq!(s.files[0].name, "a.wav");
    }

    #[test]
    fn not_a_cue_returns_none() {
        assert!(parse_cue(b"hello world").is_none());
        assert!(parse_cue(b"").is_none());
        assert!(parse_cue(&[0xff, 0x00, 0x81]).is_none());
    }

    #[test]
    fn multi_file_cue() {
        let cue = "FILE \"a.wav\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  INDEX 01 01:00:00\nFILE \"b.wav\" WAVE\n TRACK 03 AUDIO\n  INDEX 01 00:00:00\n TRACK 04 AUDIO\n  INDEX 01 02:00:00\n";
        let s = resolve(&parse_cue(cue.as_bytes()).unwrap(), "f", &[]);
        assert_eq!(s.len(), 4);
        assert_eq!(s[1].end_ms, None);
        assert_eq!(s[2].end_ms, Some(120_000));
        assert_eq!(s[3].end_ms, None);
        assert_eq!(s[2].file_name, "b.wav");
    }

    #[test]
    fn index00_ignored_missing_index01_skipped_unquoted_binary_skipped() {
        let cue = "FILE x.bin BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\nFILE a.wav WAVE\n TRACK 01 AUDIO\n  TITLE Foo\n  INDEX 00 00:00:00\n  INDEX 01 00:02:00\n TRACK 02 AUDIO\n  INDEX 00 01:00:00\n TRACK 03 AUDIO\n  INDEX 01 03:00:00\n";
        let s = parse_cue(cue.as_bytes()).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].name, "a.wav");
        assert_eq!(s.files[0].tracks.len(), 2);
        assert_eq!(s.files[0].tracks[0].start_ms, 2000 + 0);
        assert_eq!(s.files[0].tracks[0].title.as_deref(), Some("Foo"));
        assert_eq!(s.files[0].tracks[1].number, 3);
    }

    #[test]
    fn placeholders() {
        for p in ["", "Track01", "track 3", "Audio Track 1", "未知艺术家", "Unknown Artist"] {
            assert!(is_placeholder(p), "{p}");
        }
        assert!(!is_placeholder("晴天"));
        assert!(!is_placeholder("Tracks of my tears"));
    }
}
