//! Keyboard media keys and the system media controls (Windows overlay, macOS Now Playing).
//! The session starts with the first song, so an idle Linger never takes the keys from other apps.

#[cfg(any(windows, target_os = "macos"))]
pub use imp::{init, notify};

#[cfg(not(any(windows, target_os = "macos")))]
pub fn init(_app: &tauri::AppHandle) {}
#[cfg(not(any(windows, target_os = "macos")))]
pub fn notify(_app: &tauri::AppHandle) {}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
    use tauri::{AppHandle, Manager};

    use crate::commands::player::{player_next, player_prev, player_seek, player_toggle};
    use crate::state::AppState;

    #[cfg(windows)]
    const MAIN: &str = "main";
    const SEEK_STEP: f64 = 10.0;
    /// While playing, the overlay's position is corrected this often.
    const POSITION_TICK: Duration = Duration::from_secs(5);
    const IDLE_WAIT: Duration = Duration::from_secs(3600);
    const CREATE_TIMEOUT: Duration = Duration::from_secs(5);
    const FALLBACK_COVER: &[u8] = include_bytes!("../icons/128x128.png");

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Command {
        Play,
        Pause,
        Toggle,
        Next,
        Previous,
        SeekBy(f64),
        SeekTo(f64),
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Shown {
        id: String,
        title: String,
        artist: String,
        album: String,
        duration: f64,
    }

    pub struct Media {
        refresh: mpsc::Sender<()>,
    }

    fn command_for(ev: &MediaControlEvent) -> Option<Command> {
        let signed = |dir: &SeekDirection, secs: f64| if *dir == SeekDirection::Forward { secs } else { -secs };
        match ev {
            MediaControlEvent::Play => Some(Command::Play),
            // the Stop key keeps the queue and position, like Pause
            MediaControlEvent::Pause | MediaControlEvent::Stop => Some(Command::Pause),
            MediaControlEvent::Toggle => Some(Command::Toggle),
            MediaControlEvent::Next => Some(Command::Next),
            MediaControlEvent::Previous => Some(Command::Previous),
            MediaControlEvent::Seek(dir) => Some(Command::SeekBy(signed(dir, SEEK_STEP))),
            MediaControlEvent::SeekBy(dir, by) => Some(Command::SeekBy(signed(dir, by.as_secs_f64()))),
            MediaControlEvent::SetPosition(p) => Some(Command::SeekTo(p.0.as_secs_f64())),
            _ => None,
        }
    }

    fn seek_target(pos: f64, duration: f64, delta: f64) -> f64 {
        let next = (pos + delta).max(0.0);
        if duration > 0.0 { next.min(duration) } else { next }
    }

    fn should_toggle(cmd: Command, playing: bool) -> bool {
        match cmd {
            Command::Play => !playing,
            Command::Pause => playing,
            Command::Toggle => true,
            _ => false,
        }
    }

    fn span(secs: f64) -> Duration {
        if secs.is_finite() && secs > 0.0 { Duration::from_secs_f64(secs) } else { Duration::ZERO }
    }

    #[cfg(windows)]
    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    /// NSURL rejects unescaped text, and the data folder is under "Application Support".
    #[cfg(not(windows))]
    fn file_url(path: &Path) -> String {
        use std::os::unix::ffi::OsStrExt;
        let mut url = String::from("file://");
        for &b in path.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                url.push(b as char);
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
        url
    }

    /// The worker thread has no autorelease pool of its own on macOS.
    #[cfg(target_os = "macos")]
    fn pooled<T>(f: impl FnOnce() -> T) -> T {
        objc::rc::autoreleasepool(f)
    }
    #[cfg(windows)]
    fn pooled<T>(f: impl FnOnce() -> T) -> T {
        f()
    }

    fn run(app: &AppHandle, cmd: Command) {
        let h = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = h.state::<AppState>();
            let now = state.player.media_status();
            match cmd {
                Command::Next => player_next(h.clone(), state),
                Command::Previous => player_prev(h.clone(), state),
                Command::SeekBy(delta) => player_seek(seek_target(now.pos, now.duration, delta), h.clone(), state),
                Command::SeekTo(secs) => player_seek(seek_target(secs, now.duration, 0.0), h.clone(), state),
                _ if should_toggle(cmd, now.playing) => player_toggle(h.clone(), state),
                _ => {}
            }
        });
    }

    /// The session has to be created on the window's thread.
    fn create(app: &AppHandle) -> Option<MediaControls> {
        let (tx, rx) = mpsc::channel();
        let h = app.clone();
        let dispatched = app.run_on_main_thread(move || {
            let made = (|| {
                #[cfg(windows)]
                let hwnd = Some(h.get_webview_window(MAIN)?.hwnd().ok()?.0 as _);
                #[cfg(not(windows))]
                let hwnd = None;
                let config = PlatformConfig { dbus_name: "linger", display_name: "Linger", hwnd };
                let mut controls = MediaControls::new(config).map_err(|e| eprintln!("linger: media controls: {e}")).ok()?;
                let events = h.clone();
                controls
                    .attach(move |ev| {
                        if let Some(cmd) = command_for(&ev) {
                            run(&events, cmd);
                        }
                    })
                    .map_err(|e| eprintln!("linger: media keys: {e}"))
                    .ok()?;
                Some(controls)
            })();
            let _ = tx.send(made);
        });
        dispatched.ok()?;
        rx.recv_timeout(CREATE_TIMEOUT).ok().flatten()
    }

    fn fallback_cover(data_dir: &Path) -> Option<PathBuf> {
        let path = data_dir.join("media-cover.png");
        if !path.is_file() {
            std::fs::write(&path, FALLBACK_COVER).ok()?;
        }
        Some(path)
    }

    fn song_cover(state: &AppState, id: &str) -> Option<PathBuf> {
        let (song, root) = {
            let db = state.db();
            let song = db.song(id).ok().flatten()?;
            let root = db.roots().ok()?.into_iter().find(|r| r.id == song.root_id)?;
            (song, root.path)
        };
        let online = state.is_online(song.root_id);
        crate::cover::cached_cover(&state.data_dir, &root, &song, online)?;
        let name = crate::cover::cache_name(&root, &song.rel, song.size, song.mtime);
        Some(state.data_dir.join("covers").join(name)).filter(|p| p.is_file())
    }

    fn publish(controls: &mut MediaControls, s: &Shown, cover: Option<&Path>) {
        let url = cover.map(file_url);
        let meta = MediaMetadata {
            title: Some(&s.title),
            artist: Some(&s.artist),
            album: Some(&s.album),
            cover_url: url.as_deref(),
            duration: Some(span(s.duration)),
        };
        if let Err(e) = pooled(|| controls.set_metadata(meta)) {
            eprintln!("linger: media overlay: {e}");
        }
    }

    fn worker(app: AppHandle, rx: mpsc::Receiver<()>) {
        let mut controls: Option<MediaControls> = None;
        let mut unavailable = false;
        let mut shown: Option<Shown> = None;
        let mut playing = false;
        loop {
            match rx.recv_timeout(if playing { POSITION_TICK } else { IDLE_WAIT }) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while rx.try_recv().is_ok() {}
            let state = app.state::<AppState>();
            let now = state.player.media_status();
            playing = now.playing;
            let Some(id) = now.id else {
                if let Some(c) = controls.as_mut() {
                    let _ = pooled(|| c.set_playback(MediaPlayback::Stopped));
                }
                shown = None;
                continue;
            };
            if controls.is_none() && !unavailable {
                controls = create(&app);
                unavailable = controls.is_none();
            }
            let Some(c) = controls.as_mut() else { continue };
            let song = state.db().song(&id).ok().flatten();
            if let Some(song) = song {
                let next = Shown { id: id.clone(), title: song.title, artist: song.artist, album: song.album, duration: now.duration };
                if shown.as_ref() != Some(&next) {
                    // show the text right away; a song without a cover must not keep the previous one
                    publish(c, &next, fallback_cover(&state.data_dir).as_deref());
                    if let Some(cover) = song_cover(&state, &id) {
                        if state.player.current_id().as_deref() == Some(id.as_str()) {
                            publish(c, &next, Some(&cover));
                        }
                    }
                    shown = Some(next);
                }
            }
            let progress = Some(MediaPosition(span(now.pos)));
            let playback = if now.playing { MediaPlayback::Playing { progress } } else { MediaPlayback::Paused { progress } };
            if let Err(e) = pooled(|| c.set_playback(playback)) {
                eprintln!("linger: media overlay: {e}");
            }
        }
    }

    pub fn init(app: &AppHandle) {
        let (refresh, rx) = mpsc::channel();
        let h = app.clone();
        if std::thread::Builder::new().name("linger-media".into()).spawn(move || worker(h, rx)).is_ok() {
            app.manage(Media { refresh });
        }
    }

    /// The player state changed. Never blocks, so it is safe under the player lock.
    pub fn notify(app: &AppHandle) {
        if let Some(m) = app.try_state::<Media>() {
            let _ = m.refresh.send(());
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn keys_map_to_player_commands() {
            assert_eq!(command_for(&MediaControlEvent::Play), Some(Command::Play));
            assert_eq!(command_for(&MediaControlEvent::Pause), Some(Command::Pause));
            assert_eq!(command_for(&MediaControlEvent::Stop), Some(Command::Pause));
            assert_eq!(command_for(&MediaControlEvent::Next), Some(Command::Next));
            assert_eq!(command_for(&MediaControlEvent::Previous), Some(Command::Previous));
            assert_eq!(command_for(&MediaControlEvent::Seek(SeekDirection::Backward)), Some(Command::SeekBy(-SEEK_STEP)));
            assert_eq!(command_for(&MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(7)))), Some(Command::SeekTo(7.0)));
            assert_eq!(command_for(&MediaControlEvent::Quit), None);
        }

        #[test]
        fn play_and_pause_do_not_flip_the_wrong_way() {
            assert!(should_toggle(Command::Play, false));
            assert!(!should_toggle(Command::Play, true));
            assert!(should_toggle(Command::Pause, true));
            assert!(!should_toggle(Command::Pause, false));
            assert!(should_toggle(Command::Toggle, true));
            assert!(!should_toggle(Command::Next, true));
        }

        #[test]
        fn seeking_stays_inside_the_song() {
            assert_eq!(seek_target(5.0, 100.0, -10.0), 0.0);
            assert_eq!(seek_target(95.0, 100.0, 10.0), 100.0);
            assert_eq!(seek_target(20.0, 100.0, 10.0), 30.0);
            assert_eq!(seek_target(20.0, 0.0, 10.0), 30.0);
        }

        #[test]
        fn bad_times_become_zero() {
            assert_eq!(span(f64::NAN), Duration::ZERO);
            assert_eq!(span(-1.0), Duration::ZERO);
            assert_eq!(span(1.5), Duration::from_millis(1500));
        }

        #[cfg(windows)]
        #[test]
        fn cover_path_is_a_file_url() {
            assert_eq!(file_url(Path::new(r"C:\data\covers\a.jpg")), r"file://C:\data\covers\a.jpg");
        }

        #[cfg(not(windows))]
        #[test]
        fn cover_path_is_an_escaped_file_url() {
            assert_eq!(file_url(Path::new("/Users/a/Application Support/封.jpg")), "file:///Users/a/Application%20Support/%E5%B0%81.jpg");
        }

        #[test]
        fn fallback_cover_is_written_once() {
            let t = tempfile::tempdir().unwrap();
            let p = fallback_cover(t.path()).unwrap();
            assert_eq!(std::fs::read(&p).unwrap(), FALLBACK_COVER);
            std::fs::write(&p, b"kept").unwrap();
            fallback_cover(t.path()).unwrap();
            assert_eq!(std::fs::read(&p).unwrap(), b"kept");
        }
    }
}
