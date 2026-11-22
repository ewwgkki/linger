//! Exports a playlist or the liked songs as an M3U8 file. Never writes into a music folder.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use super::player::song_path;
use crate::db::{Db, Root};
use crate::state::AppState;

const EMPTY: &str = "export.empty";
const IN_LIBRARY: &str = "export.insideLibrary";
const WRITE_FAILED: &str = "export.writeFailed";
const NOT_FOUND: &str = "playlist.notFound";
const MAX_FILE_STEM: usize = 80;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub path: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportDto {
    pub file: String,
    pub exported: usize,
    pub skipped: usize,
}

/// A Windows-safe `<name>.m3u8` file name.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .take(MAX_FILE_STEM)
        .collect();
    let mut stem = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if stem.is_empty() {
        stem = "playlist".into();
    }
    let base = stem.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        stem.insert(0, '_');
    }
    format!("{stem}.m3u8")
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// Extended M3U in UTF-8. CUE tracks carry start and stop times for players that read them.
pub fn render_m3u8(name: &str, entries: &[Entry]) -> String {
    let mut out = format!("#EXTM3U\r\n#PLAYLIST:{}\r\n", one_line(name));
    for e in entries {
        let title = one_line(&e.title);
        let artist = one_line(&e.artist);
        let label = if artist.is_empty() { title } else { format!("{artist} - {title}") };
        out.push_str(&format!("#EXTINF:{},{label}\r\n", e.duration.max(0.0).round() as i64));
        if e.start_ms > 0 || e.end_ms.is_some() {
            out.push_str(&format!("#EXTVLCOPT:start-time={:.3}\r\n", e.start_ms as f64 / 1000.0));
            if let Some(end) = e.end_ms {
                out.push_str(&format!("#EXTVLCOPT:stop-time={:.3}\r\n", end as f64 / 1000.0));
            }
        }
        out.push_str(&one_line(&e.path));
        out.push_str("\r\n");
    }
    out
}

fn norm(s: &str) -> String {
    s.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Whether `target` is `root` or below it (Windows path rules, ignoring case).
pub fn is_inside(target: &str, root: &str) -> bool {
    let (t, r) = (norm(target), norm(root));
    !r.is_empty() && (t == r || t.starts_with(&format!("{r}\\")))
}

/// Compares written paths and resolved ones, so a mapped drive and its UNC path are the same.
fn lands_in_library(target: &Path, roots: &[Root]) -> bool {
    let text = target.to_string_lossy();
    let parent = target.parent().and_then(|p| std::fs::canonicalize(p).ok());
    roots.iter().any(|r| {
        is_inside(&text, &r.path)
            || match (&parent, std::fs::canonicalize(&r.path)) {
                (Some(p), Ok(root)) => is_inside(&p.to_string_lossy(), &root.to_string_lossy()),
                _ => false,
            }
    })
}

/// Current library entries for the songs; songs no longer in the library are counted as skipped.
pub fn collect(db: &Db, song_ids: &[String]) -> Result<(Vec<Entry>, usize), String> {
    let roots = db.roots().map_err(|e| e.to_string())?;
    let mut entries = Vec::with_capacity(song_ids.len());
    let mut skipped = 0;
    for id in song_ids {
        let song = db.song(id).map_err(|e| e.to_string())?;
        match song.and_then(|s| song_path(&roots, &s).map(|p| (s, p))) {
            Some((s, p)) => entries.push(Entry {
                title: s.title,
                artist: s.artist,
                duration: s.duration,
                path: p.to_string_lossy().into_owned(),
                start_ms: s.start_ms,
                end_ms: s.end_ms,
            }),
            None => skipped += 1,
        }
    }
    Ok((entries, skipped))
}

fn write_file(target: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `id`: a playlist, or `None` for the liked songs (named `fallback_name`).
/// Returns `None` when the save dialog is cancelled.
#[tauri::command(async)]
pub fn export_playlist(
    id: Option<i64>,
    fallback_name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ExportDto>, String> {
    let (name, entries, skipped, roots) = {
        let db = state.db();
        let (name, song_ids): (String, Vec<String>) = match id {
            Some(pl) => {
                let list = db.playlists().map_err(|e| e.to_string())?.into_iter().find(|p| p.id == pl);
                let name = list.ok_or(NOT_FOUND)?.name;
                let items = db.playlist_items(pl).map_err(|e| e.to_string())?;
                (name, items.into_iter().map(|i| i.song_id).collect())
            }
            None => {
                let favs = db.favorites().map_err(|e| e.to_string())?;
                (fallback_name, favs.into_iter().map(|f| f.song_id).collect())
            }
        };
        let (entries, skipped) = collect(&db, &song_ids)?;
        (name, entries, skipped, db.roots().map_err(|e| e.to_string())?)
    };
    if entries.is_empty() {
        return Err(EMPTY.into());
    }

    let mut dialog = app.dialog().file().add_filter("M3U8", &["m3u8"]).set_file_name(safe_file_name(&name));
    if let Ok(dir) = app.path().document_dir() {
        dialog = dialog.set_directory(dir);
    }
    if let Some(w) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&w);
    }
    let Some(picked) = dialog.blocking_save_file() else { return Ok(None) };
    let mut target = picked.into_path().map_err(|_| WRITE_FAILED.to_string())?;
    if !target.extension().is_some_and(|e| e.eq_ignore_ascii_case("m3u8")) {
        target.as_mut_os_string().push(".m3u8");
    }
    if lands_in_library(&target, &roots) {
        return Err(IN_LIBRARY.into());
    }
    write_file(&target, &render_m3u8(&name, &entries)).map_err(|_| WRITE_FAILED.to_string())?;
    let file = target.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(ExportDto { file, exported: entries.len(), skipped }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            title: "Song".into(),
            artist: "Artist".into(),
            duration: 181.6,
            path: r"D:\Music\Song.flac".into(),
            start_ms: 0,
            end_ms: None,
        }
    }

    #[test]
    fn renders_extended_m3u() {
        let text = render_m3u8("Late night", &[entry()]);
        assert_eq!(text, "#EXTM3U\r\n#PLAYLIST:Late night\r\n#EXTINF:182,Artist - Song\r\nD:\\Music\\Song.flac\r\n");
    }

    #[test]
    fn cue_tracks_keep_their_range() {
        let e = Entry { start_ms: 12_345, end_ms: Some(60_000), ..entry() };
        let text = render_m3u8("x", &[e]);
        assert!(text.contains("#EXTVLCOPT:start-time=12.345\r\n#EXTVLCOPT:stop-time=60.000\r\nD:"));
    }

    #[test]
    fn tags_cannot_inject_lines() {
        let e = Entry { title: "A\r\n#EXTM3U\nB".into(), ..entry() };
        let text = render_m3u8("na\nme", &[e]);
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn file_names_are_windows_safe() {
        assert_eq!(safe_file_name("Rock: A/B?"), "Rock_ A_B_.m3u8");
        assert_eq!(safe_file_name("  ...  "), "playlist.m3u8");
        assert_eq!(safe_file_name("CON"), "_CON.m3u8");
        assert_eq!(safe_file_name("com1.live"), "_com1.live.m3u8");
        assert_eq!(safe_file_name("夜晚"), "夜晚.m3u8");
        assert_eq!(safe_file_name(&"歌".repeat(200)).chars().count(), MAX_FILE_STEM + 5);
    }

    #[test]
    fn detects_targets_inside_a_music_folder() {
        assert!(is_inside(r"D:\Music\a.m3u8", r"D:\Music"));
        assert!(is_inside(r"d:/music/sub/a.m3u8", r"D:\Music\"));
        assert!(is_inside(r"\\NAS\share\a.m3u8", r"\\nas\SHARE"));
        assert!(is_inside(r"D:\a.m3u8", r"D:\"));
        assert!(!is_inside(r"D:\Music2\a.m3u8", r"D:\Music"));
        assert!(!is_inside(r"C:\Users\a.m3u8", r"D:\Music"));
    }
}
