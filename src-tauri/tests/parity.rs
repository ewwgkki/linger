//! Parity check of `tags::read_audio_info` against the web version's scanned library.
//!
//! Needs real data, so it is ignored by default:
//!   LINGER_GOLDEN   path of the web version's data/library.php (PHP guard line + JSON)
//!   LINGER_ROOTMAP  "<server prefix>=<local prefix>[;...]", e.g. `/volume1/music=\\NAS\music`
//! Run: cargo test --test parity -- --ignored --nocapture

use std::path::PathBuf;

use linger_lib::derive::{derive, RawTags};
use linger_lib::tags::read_audio_info;
use serde_json::Value;

fn map_root(root: &str, maps: &[(String, String)]) -> String {
    for (from, to) in maps {
        if let Some(rest) = root.strip_prefix(from.as_str()) {
            return format!("{to}{rest}");
        }
    }
    root.to_string()
}

fn local_path(root: &str, rel: &str) -> PathBuf {
    let p = format!("{root}/{rel}");
    PathBuf::from(if cfg!(windows) { p.replace('/', "\\") } else { p })
}

#[test]
#[ignore]
fn parity_with_web_library() {
    let golden = std::env::var("LINGER_GOLDEN").expect("set LINGER_GOLDEN");
    let rootmap = std::env::var("LINGER_ROOTMAP").unwrap_or_default();
    let maps: Vec<(String, String)> = rootmap
        .split(';')
        .filter_map(|m| m.split_once('='))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();

    let raw = std::fs::read_to_string(&golden).expect("read golden");
    let json = match raw.find('\n') {
        Some(i) if raw.starts_with("<?php") => &raw[i + 1..],
        _ => &raw[..],
    };
    let lib: Value = serde_json::from_str(json).expect("golden JSON");
    let roots: Vec<String> = lib["roots"].as_array().expect("roots").iter()
        .map(|r| map_root(r.as_str().unwrap_or_default(), &maps))
        .collect();
    let songs = lib["songs"].as_array().expect("songs");

    let mut bad = 0usize;
    for (n, s) in songs.iter().enumerate() {
        let root = &roots[s["r"].as_u64().unwrap_or(0) as usize];
        let rel = s["p"].as_str().unwrap_or_default();
        let path = local_path(root, rel);
        let i = read_audio_info(&path);
        let tag = &s["tag"];
        let st = |k: &str| tag[k].as_str().unwrap_or_default().to_string();
        let mut diffs: Vec<String> = Vec::new();
        let mut cmp = |name: &str, got: String, want: String| {
            if got != want {
                diffs.push(format!("{name}: got {got:?} want {want:?}"));
            }
        };
        cmp("codec", i.codec.clone(), s["c"].as_str().unwrap_or_default().to_string());
        cmp("bitrate", i.bitrate.to_string(), s["br"].as_i64().unwrap_or(0).to_string());
        cmp("secs", (i.duration.round() as i64).to_string(), s["s"].as_i64().unwrap_or(0).to_string());
        cmp("pic", i.pic.to_string(), (s["pc"].as_i64() == Some(1)).to_string());
        cmp("title", i.title.clone(), st("title"));
        cmp("artist", i.artist.clone(), st("artist"));
        cmp("album", i.album.clone(), st("album"));
        cmp("albumartist", i.albumartist.clone(), st("albumartist"));
        cmp("track", i.track.to_string(), tag["track"].as_i64().unwrap_or(0).to_string());
        let raw = RawTags {
            title: i.title.clone(), artist: i.artist.clone(), album: i.album.clone(),
            albumartist: i.albumartist.clone(), track: i.track,
        };
        let dv = derive(rel, &raw, i.duration);
        cmp("derive.t", dv.title, s["t"].as_str().unwrap_or_default().to_string());
        cmp("derive.a", dv.artist, s["a"].as_str().unwrap_or_default().to_string());
        cmp("derive.b", dv.album, s["b"].as_str().unwrap_or_default().to_string());
        cmp("derive.n", dv.track.to_string(), s["n"].as_i64().unwrap_or(0).to_string());
        if !diffs.is_empty() {
            bad += 1;
            println!("MISMATCH #{n} {}\n    {}", path.display(), diffs.join("\n    "));
        }
    }
    let total = songs.len();
    println!("parity: {bad}/{total} songs differ ({:.2}%)", bad as f64 * 100.0 / total.max(1) as f64);
    assert!(total > 0, "golden library has no songs");
    assert!(bad * 100 <= total, "more than 1% of songs differ");
}
