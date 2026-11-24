//! System tray (menu bar on macOS): closing the window keeps Linger playing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, Window, WindowEvent, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::commands::player::{player_next, player_prev, player_toggle};
use crate::state::AppState;

const MAIN: &str = "main";
const CLOSE_KEY: &str = "close_to_tray";
const HINT_KEY: &str = "tray_hint_shown";
/// A click on the tray takes focus from the window just before the click arrives.
const BLUR_GRACE: Duration = Duration::from_millis(400);
/// The two clicks of a double-click must not show and immediately hide the window.
const CLICK_DEBOUNCE: Duration = Duration::from_millis(400);
/// Windows limits tray tooltips to 127 UTF-16 units.
const TOOLTIP_CHARS: usize = 60;
const MENU_CHARS: usize = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Labels {
    idle: &'static str,
    play: &'static str,
    pause: &'static str,
    prev: &'static str,
    next: &'static str,
    show: &'static str,
    quit: &'static str,
    hint: &'static str,
}

fn labels(lang: &str) -> Labels {
    if lang.starts_with("zh") {
        Labels {
            idle: "没有正在播放的歌曲",
            play: "播放",
            pause: "暂停",
            prev: "上一首",
            next: "下一首",
            show: "显示 Linger",
            quit: "退出",
            hint: if cfg!(target_os = "macos") {
                "Linger 仍在后台运行。点 Dock 图标可以重新打开，也可以从屏幕顶部菜单栏的图标退出。"
            } else {
                "Linger 仍在后台运行。可以从任务栏右下角的托盘图标打开或退出；如果没看到图标，请点右下角的 ^。"
            },
        }
    } else {
        Labels {
            idle: "Nothing playing",
            play: "Play",
            pause: "Pause",
            prev: "Previous",
            next: "Next",
            show: "Show Linger",
            quit: "Quit",
            hint: if cfg!(target_os = "macos") {
                "Linger is still running. Click its Dock icon to reopen it, or quit from its icon in the menu bar."
            } else {
                "Linger is still running. Open or quit it from the tray icon at the bottom right of the taskbar. If you can't see the icon, click ^ there."
            },
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn song_label(title: &str, artist: &str) -> String {
    if artist.is_empty() { title.to_string() } else { format!("{title} - {artist}") }
}

fn tooltip(song: Option<&str>) -> String {
    song.map_or_else(|| "Linger".to_string(), |s| clip(s, TOOLTIP_CHARS))
}

/// Windows treats `&` in menu text as a keyboard-accelerator marker.
fn menu_text(s: &str) -> String {
    clip(s, MENU_CHARS).replace('&', "&&")
}

fn should_hide(visible: bool, minimized: bool, focused: bool, since_blur: Option<Duration>) -> bool {
    visible && !minimized && (focused || since_blur.is_some_and(|d| d < BLUR_GRACE))
}

fn close_enabled(saved: Option<&str>) -> bool {
    saved != Some("false")
}

struct Items {
    now: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    prev: MenuItem<Wry>,
    next: MenuItem<Wry>,
    show: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

pub struct Tray {
    close_to_tray: AtomicBool,
    quitting: AtomicBool,
    last_blur: Mutex<Option<Instant>>,
    last_click: Mutex<Option<Instant>>,
    lang: Mutex<String>,
    refresh: mpsc::Sender<()>,
    items: Items,
    icon: TrayIcon,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Creates the tray icon; call after `AppState` is managed and the player started.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let l = labels("en");
    let items = Items {
        now: MenuItem::with_id(app, "now", l.idle, false, None::<&str>)?,
        toggle: MenuItem::with_id(app, "toggle", l.play, false, None::<&str>)?,
        prev: MenuItem::with_id(app, "prev", l.prev, false, None::<&str>)?,
        next: MenuItem::with_id(app, "next", l.next, false, None::<&str>)?,
        show: MenuItem::with_id(app, "show", l.show, true, None::<&str>)?,
        quit: MenuItem::with_id(app, "quit", l.quit, true, None::<&str>)?,
    };
    let menu = Menu::with_items(
        app,
        &[
            &items.now,
            &PredefinedMenuItem::separator(app)?,
            &items.toggle,
            &items.prev,
            &items.next,
            &PredefinedMenuItem::separator(app)?,
            &items.show,
            &items.quit,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id(MAIN)
        .menu(&menu)
        // a menu bar icon opens its menu on click; the Windows tray icon toggles the window
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .tooltip("Linger")
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, ev| on_icon(tray.app_handle(), ev));
    #[cfg(target_os = "macos")]
    {
        match menu_bar_icon() {
            Some(i) => builder = builder.icon(i).icon_as_template(true),
            None => eprintln!("linger: cannot read the menu bar icon"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(i) = app.default_window_icon() {
        builder = builder.icon(i.clone());
    }
    let icon = builder.build(app)?;

    let close_to_tray = {
        let state = app.state::<AppState>();
        let saved = state.db().setting(CLOSE_KEY).ok().flatten();
        close_enabled(saved.as_deref())
    };
    let (refresh, rx) = mpsc::channel();
    app.manage(Tray {
        close_to_tray: AtomicBool::new(close_to_tray),
        quitting: AtomicBool::new(false),
        last_blur: Mutex::new(None),
        last_click: Mutex::new(None),
        lang: Mutex::new("en".into()),
        refresh,
        items,
        icon,
    });
    let h = app.clone();
    std::thread::Builder::new().name("linger-tray".into()).spawn(move || refresh_loop(h, rx))?;
    Ok(())
}

/// A black-and-alpha image, so macOS can tint it for light and dark menu bars.
#[cfg(target_os = "macos")]
fn menu_bar_icon() -> Option<tauri::image::Image<'static>> {
    let img = image::load_from_memory(include_bytes!("../icons/menu-bar-template.png")).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(tauri::image::Image::new_owned(img.into_raw(), w, h))
}

/// Menu updates wait on the main thread, so they run here with no app lock held.
fn refresh_loop(app: AppHandle, rx: mpsc::Receiver<()>) {
    let mut shown: Option<(Option<String>, bool, String)> = None;
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        let Some(tray) = app.try_state::<Tray>() else { continue };
        let state = app.state::<AppState>();
        let (id, playing) = state.player.status();
        let lang = lock(&tray.lang).clone();
        let next = (id.clone(), playing, lang.clone());
        if shown.as_ref() == Some(&next) {
            continue;
        }
        let song = id.as_deref().and_then(|i| state.db().song(i).ok().flatten()).map(|s| song_label(&s.title, &s.artist));
        let l = labels(&lang);
        let i = &tray.items;
        let has_song = id.is_some();
        let _ = i.now.set_text(song.as_deref().map_or_else(|| l.idle.to_string(), menu_text));
        let _ = i.toggle.set_text(if playing { l.pause } else { l.play });
        let _ = i.prev.set_text(l.prev);
        let _ = i.next.set_text(l.next);
        let _ = i.show.set_text(l.show);
        let _ = i.quit.set_text(l.quit);
        for item in [&i.toggle, &i.prev, &i.next] {
            let _ = item.set_enabled(has_song);
        }
        let _ = tray.icon.set_tooltip(Some(tooltip(song.as_deref())));
        shown = Some(next);
    }
}

/// The player state changed.
pub fn notify(app: &AppHandle) {
    if let Some(t) = app.try_state::<Tray>() {
        let _ = t.refresh.send(());
    }
}

pub fn set_language(app: &AppHandle, lang: &str) {
    if let Some(t) = app.try_state::<Tray>() {
        *lock(&t.lang) = lang.to_string();
        let _ = t.refresh.send(());
    }
}

fn on_menu(app: &AppHandle, ev: MenuEvent) {
    let id = ev.id().as_ref().to_string();
    match id.as_str() {
        "show" => show(app),
        "quit" => quit(app),
        "toggle" | "prev" | "next" => {
            let h = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let state = h.state::<AppState>();
                match id.as_str() {
                    "toggle" => player_toggle(h.clone(), state),
                    "prev" => player_prev(h.clone(), state),
                    _ => player_next(h.clone(), state),
                }
            });
        }
        _ => {}
    }
}

fn on_icon(app: &AppHandle, ev: TrayIconEvent) {
    if cfg!(target_os = "macos") {
        return;
    }
    let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev else { return };
    let Some(t) = app.try_state::<Tray>() else { return };
    {
        let mut last = lock(&t.last_click);
        if last.is_some_and(|i| i.elapsed() < CLICK_DEBOUNCE) {
            return;
        }
        *last = Some(Instant::now());
    }
    let Some(w) = app.get_webview_window(MAIN) else { return };
    let since_blur = lock(&t.last_blur).map(|i| i.elapsed());
    let hide = should_hide(
        w.is_visible().unwrap_or(false),
        w.is_minimized().unwrap_or(false),
        w.is_focused().unwrap_or(false),
        since_blur,
    );
    if hide {
        let _ = w.hide();
    } else {
        show(app);
    }
}

/// Brings the window back and tells the page to catch up.
pub fn show(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
    let _ = app.emit("app://shown", ());
}

pub fn on_window_event(window: &Window, ev: &WindowEvent) {
    if window.label() != MAIN {
        return;
    }
    let app = window.app_handle();
    // without a tray icon the window keeps its normal close behavior
    let Some(t) = app.try_state::<Tray>() else { return };
    match ev {
        WindowEvent::Focused(false) => *lock(&t.last_blur) = Some(Instant::now()),
        WindowEvent::CloseRequested { api, .. } if !t.quitting.load(Ordering::SeqCst) => {
            api.prevent_close();
            if t.close_to_tray.load(Ordering::SeqCst) {
                let _ = window.hide();
                first_close_hint(app);
            } else {
                quit(app);
            }
        }
        _ => {}
    }
}

/// macOS only: the Dock icon reopens a hidden window, and Cmd+Q must save and stop like Quit.
pub fn on_run_event(app: &AppHandle, ev: RunEvent) {
    #[cfg(target_os = "macos")]
    match ev {
        RunEvent::Reopen { .. } => show(app),
        RunEvent::Exit => {
            let quitting = app.try_state::<Tray>().is_some_and(|t| t.quitting.load(Ordering::SeqCst));
            if !quitting && app.try_state::<AppState>().is_some() {
                prepare_exit(app);
            }
        }
        _ => {}
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, ev);
}

fn first_close_hint(app: &AppHandle) {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = h.state::<AppState>();
        {
            let db = state.db();
            if db.setting(HINT_KEY).ok().flatten().is_some() {
                return;
            }
            let _ = db.set_setting(HINT_KEY, "true");
        }
        let Some(t) = h.try_state::<Tray>() else { return };
        let lang = lock(&t.lang).clone();
        h.dialog().message(labels(&lang).hint).title("Linger").kind(MessageDialogKind::Info).show(|_| {});
    });
}

/// Gets ready for the process to end: stops playback and removes the tray icon.
pub fn prepare_exit(app: &AppHandle) {
    if let Some(t) = app.try_state::<Tray>() {
        t.quitting.store(true, Ordering::SeqCst);
        let _ = t.icon.set_visible(false);
    }
    crate::resume::save_now(app);
    let state = app.state::<AppState>();
    state.player.shutdown();
    let locked = state.db.try_lock();
    if let Ok(db) = &locked {
        let _ = db.checkpoint();
    }
}

/// The exit did not happen (a failed update): the app keeps running.
pub fn cancel_exit(app: &AppHandle) {
    if let Some(t) = app.try_state::<Tray>() {
        t.quitting.store(false, Ordering::SeqCst);
        let _ = t.icon.set_visible(true);
    }
}

/// Really exits: stops playback (and any FFmpeg child) first.
pub fn quit(app: &AppHandle) {
    let Some(t) = app.try_state::<Tray>() else {
        app.exit(0);
        return;
    };
    if t.quitting.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
    let h = app.clone();
    let spawned = std::thread::Builder::new().name("linger-quit".into()).spawn(move || {
        crate::resume::save_now(&h);
        let state = h.state::<AppState>();
        state.player.shutdown();
        // a running scan may hold the database: its writes are already crash-safe
        if let Ok(db) = state.db.try_lock() {
            let _ = db.checkpoint();
        }
        h.exit(0);
    });
    if spawned.is_err() {
        app.exit(0);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowSettings {
    pub close_to_tray: bool,
}

#[tauri::command(async)]
pub fn window_settings(app: AppHandle) -> WindowSettings {
    let close_to_tray = app.try_state::<Tray>().is_some_and(|t| t.close_to_tray.load(Ordering::SeqCst));
    WindowSettings { close_to_tray }
}

#[tauri::command(async)]
pub fn window_set_close_to_tray(enabled: bool, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.db().set_setting(CLOSE_KEY, if enabled { "true" } else { "false" }).map_err(|e| e.to_string())?;
    if let Some(t) = app.try_state::<Tray>() {
        t.close_to_tray.store(enabled, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_language() {
        assert_eq!(labels("zh-CN").quit, "退出");
        assert_eq!(labels("en").quit, "Quit");
    }

    #[test]
    fn close_to_tray_defaults_on() {
        assert!(close_enabled(None));
        assert!(close_enabled(Some("true")));
        assert!(!close_enabled(Some("false")));
    }

    #[test]
    fn click_hides_only_a_window_in_front() {
        let recent = Some(Duration::from_millis(100));
        let old = Some(Duration::from_secs(5));
        assert!(should_hide(true, false, true, None));
        assert!(should_hide(true, false, false, recent));
        assert!(!should_hide(true, false, false, old));
        assert!(!should_hide(true, true, true, recent));
        assert!(!should_hide(false, false, false, recent));
    }

    #[test]
    fn text_is_safe_for_windows_menus() {
        assert_eq!(song_label("Song", "Artist"), "Song - Artist");
        assert_eq!(song_label("Song", ""), "Song");
        assert_eq!(menu_text("Rock & Roll"), "Rock && Roll");
        let long = "歌".repeat(200);
        assert_eq!(tooltip(Some(&long)).chars().count(), TOOLTIP_CHARS);
        assert_eq!(tooltip(None), "Linger");
    }
}
