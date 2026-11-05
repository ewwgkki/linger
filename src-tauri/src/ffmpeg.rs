//! Locating an ffmpeg executable (used for the library flag and by the decoder).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const EXE: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

/// ffmpeg next to the app exe; in debug builds `src-tauri/binaries/ffmpeg*`; then PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let dev_bin = cfg!(debug_assertions).then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries"));
    find_in(exe_dir.as_deref(), dev_bin.as_deref(), std::env::var_os("PATH").as_deref())
}

fn find_in(exe_dir: Option<&Path>, dev_bin: Option<&Path>, path_var: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(p) = exe_dir.map(|d| d.join(EXE)).filter(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(dir) = dev_bin {
        // Tauri sidecar naming: ffmpeg-<target triple>.exe
        let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                n.starts_with("ffmpeg") && (!cfg!(windows) || n.ends_with(".exe"))
            })
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        hits.sort();
        if let Some(p) = hits.into_iter().next() {
            return Some(p);
        }
    }
    std::env::split_paths(path_var?).map(|d| d.join(EXE)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn search_order() {
        let t = tempfile::tempdir().unwrap();
        let (exe, dev, p1, p2) = (t.path().join("exe"), t.path().join("dev"), t.path().join("p1"), t.path().join("p2"));
        for d in [&exe, &dev, &p1, &p2] {
            fs::create_dir_all(d).unwrap();
        }
        let path_var = std::env::join_paths([&p1, &p2]).unwrap();
        let find = || find_in(Some(&exe), Some(&dev), Some(&path_var));
        assert_eq!(find(), None);
        fs::write(p2.join(EXE), b"x").unwrap();
        assert_eq!(find(), Some(p2.join(EXE)));
        fs::write(p1.join(EXE), b"x").unwrap();
        assert_eq!(find(), Some(p1.join(EXE)));
        let sidecar = if cfg!(windows) { "ffmpeg-x86_64-pc-windows-msvc.exe" } else { "ffmpeg-x86_64-unknown-linux-gnu" };
        fs::write(dev.join("readme.txt"), b"x").unwrap();
        fs::write(dev.join(sidecar), b"x").unwrap();
        assert_eq!(find(), Some(dev.join(sidecar)));
        fs::write(exe.join(EXE), b"x").unwrap();
        assert_eq!(find(), Some(exe.join(EXE)));
        assert_eq!(find_in(None, None, None), None);
    }

    #[test]
    fn real_lookup_does_not_panic() {
        let _ = find_ffmpeg();
    }
}
