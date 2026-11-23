//! In-app updates from GitHub releases. Packages are verified with the updater signature and
//! are only installed when the user confirms.

use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

const AUTO_KEY: &str = "update_auto";
const LAST_KEY: &str = "update_last_check";
const KNOWN_KEY: &str = "update_known_version";
const DAY_SECS: i64 = 24 * 60 * 60;
const CHECK_FAILED: &str = "update.checkFailed";
const INSTALL_FAILED: &str = "update.installFailed";
const NOT_READY: &str = "update.notReady";

#[derive(Default)]
pub struct UpdateState {
    pending: Mutex<Option<Update>>,
    bytes: Mutex<Option<Vec<u8>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateSettings {
    pub auto: bool,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    pct: u8,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Automatic checks run at most daily, or when a newer version is already known.
fn should_auto_check(auto: bool, now: i64, last: Option<i64>, known_newer: bool) -> bool {
    auto && (known_newer || last.is_none_or(|l| now.saturating_sub(l) >= DAY_SECS || l > now))
}

fn percent(done: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|t| *t > 0)?;
    Some(((done.min(total) as u128 * 100) / total as u128) as u8)
}

fn auto_enabled(saved: Option<&str>) -> bool {
    saved != Some("false")
}

#[tauri::command(async)]
pub fn update_settings(app: AppHandle) -> UpdateSettings {
    let state = app.state::<AppState>();
    let saved = state.db().setting(AUTO_KEY).ok().flatten();
    UpdateSettings { auto: auto_enabled(saved.as_deref()), version: app.package_info().version.to_string() }
}

#[tauri::command(async)]
pub fn update_set_auto(enabled: bool, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let result = state.db().set_setting(AUTO_KEY, if enabled { "true" } else { "false" });
    result.map_err(|e| e.to_string())
}

/// `manual: false` is the quiet background check; it may decide not to contact GitHub.
#[tauri::command]
pub async fn update_check(manual: bool, app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let current = app.package_info().version.to_string();
    if !manual {
        let state = app.state::<AppState>();
        let db = state.db();
        let auto = auto_enabled(db.setting(AUTO_KEY).ok().flatten().as_deref());
        let last = db.setting(LAST_KEY).ok().flatten().and_then(|v| v.parse().ok());
        let known = db.setting(KNOWN_KEY).ok().flatten().is_some_and(|v| is_newer(&v, &current));
        if !should_auto_check(auto, now(), last, known) {
            return Ok(None);
        }
    }
    let updater = app.updater().map_err(|e| {
        eprintln!("linger: updater: {e}");
        CHECK_FAILED.to_string()
    })?;
    let found = updater.check().await.map_err(|e| {
        eprintln!("linger: update check: {e}");
        CHECK_FAILED.to_string()
    })?;
    {
        let state = app.state::<AppState>();
        let db = state.db();
        let _ = db.set_setting(LAST_KEY, &now().to_string());
        let _ = db.set_setting(KNOWN_KEY, found.as_ref().map_or("", |u| u.version.as_str()));
    }
    let st = app.state::<UpdateState>();
    let info = found.as_ref().map(|u| UpdateInfo { version: u.version.clone(), notes: u.body.clone().unwrap_or_default() });
    let same = lock(&st.pending).as_ref().map(|u| u.version.clone()) == found.as_ref().map(|u| u.version.clone());
    if !same {
        *lock(&st.bytes) = None;
    }
    *lock(&st.pending) = found;
    Ok(info)
}

/// Starts the download. Progress arrives as "update://progress", then "update://ready" or "update://failed".
#[tauri::command]
pub fn update_download(app: AppHandle) -> Result<(), String> {
    let st = app.state::<UpdateState>();
    let update = lock(&st.pending).clone().ok_or(NOT_READY)?;
    if lock(&st.bytes).is_some() {
        let _ = app.emit("update://ready", ());
        return Ok(());
    }
    let mut task = lock(&st.task);
    if task.is_some() {
        return Ok(());
    }
    let h = app.clone();
    *task = Some(tauri::async_runtime::spawn(async move {
        let progress = h.clone();
        let (mut done, mut shown) = (0u64, None);
        let result = update
            .download(
                move |chunk, total| {
                    done += chunk as u64;
                    let pct = percent(done, total);
                    if pct.is_some() && pct != shown {
                        shown = pct;
                        let _ = progress.emit("update://progress", Progress { pct: pct.unwrap_or(0) });
                    }
                },
                || {},
            )
            .await;
        let st = h.state::<UpdateState>();
        *lock(&st.task) = None;
        match result {
            Ok(bytes) => {
                *lock(&st.bytes) = Some(bytes);
                let _ = h.emit("update://ready", ());
            }
            Err(e) => {
                eprintln!("linger: update download: {e}");
                let _ = h.emit("update://failed", ());
            }
        }
    }));
    Ok(())
}

#[tauri::command]
pub fn update_cancel(app: AppHandle) {
    let st = app.state::<UpdateState>();
    let task = lock(&st.task).take();
    if let Some(t) = task {
        t.abort();
    }
}

/// Stops playback and runs the verified installer, which reopens Linger when it finishes.
#[tauri::command(async)]
pub fn update_install(app: AppHandle) -> Result<(), String> {
    let st = app.state::<UpdateState>();
    let update = lock(&st.pending).clone().ok_or(NOT_READY)?;
    let bytes = lock(&st.bytes).take().ok_or(NOT_READY)?;
    crate::tray::prepare_exit(&app);
    if let Err(e) = update.install(&bytes) {
        eprintln!("linger: update install: {e}");
        *lock(&st.bytes) = Some(bytes);
        crate::tray::cancel_exit(&app);
        return Err(INSTALL_FAILED.into());
    }
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.6.0"));
        assert!(!is_newer("0.6.0", "0.6.0"));
        assert!(!is_newer("0.5.9", "0.6.0"));
        assert!(!is_newer("garbage", "0.6.0"));
        assert!(!is_newer("", "0.6.0"));
    }

    #[test]
    fn automatic_checks_are_daily() {
        assert!(should_auto_check(true, 1000, None, false));
        assert!(!should_auto_check(true, DAY_SECS, Some(1), false));
        assert!(should_auto_check(true, DAY_SECS + 1, Some(1), false));
        assert!(should_auto_check(true, 10, Some(9), true));
        assert!(should_auto_check(true, 10, Some(5000), false));
        assert!(!should_auto_check(false, DAY_SECS * 9, Some(1), true));
    }

    #[test]
    fn progress_is_bounded() {
        assert_eq!(percent(50, Some(200)), Some(25));
        assert_eq!(percent(300, Some(200)), Some(100));
        assert_eq!(percent(1, None), None);
        assert_eq!(percent(1, Some(0)), None);
    }

    #[test]
    fn automatic_checks_default_on() {
        assert!(auto_enabled(None));
        assert!(!auto_enabled(Some("false")));
    }
}
