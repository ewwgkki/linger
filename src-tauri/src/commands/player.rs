//! Player commands and events. The queue lives here (not in the webview) so media keys and
//! the tray (P4) drive the same state.
//!
//! Events: "player://state" PlayerStateDto, "player://pos" { pos }, "player://error" { id, reason }.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::db::{Db, Root, SongRow};
use crate::player::decode::DecodeError;
use crate::player::engine::{Engine, EngineEvent};
use crate::player::queue::{Queue, Repeat};
use crate::state::AppState;

const SKIP_DELAY: Duration = Duration::from_millis(1200);
/// `prev` past this point restarts the song instead
const PREV_RESTART_SECS: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    Stop,
    SkipAfter(Duration),
}

/// `failures`: consecutive failures including this one.
pub fn on_error(failures: usize, queue_len: usize) -> ErrorAction {
    if failures >= queue_len {
        ErrorAction::Stop
    } else {
        ErrorAction::SkipAfter(SKIP_DELAY)
    }
}

/// i18n key for a playback failure.
pub fn error_reason(e: &DecodeError) -> &'static str {
    match e {
        DecodeError::NotFound => "player.notFound",
        DecodeError::NeedFfmpeg => "player.needFfmpeg",
        DecodeError::Unsupported => "player.unsupported",
        DecodeError::Io(_) => "player.io",
        DecodeError::Corrupt(_) => "player.corrupt",
    }
}
const NO_OUTPUT: &str = "player.noOutput";

/// The root's path joined with the '/'-separated relative path.
pub fn song_path(roots: &[Root], s: &SongRow) -> Option<PathBuf> {
    let root = roots.iter().find(|r| r.id == s.root_id)?;
    Some(s.rel.split('/').filter(|p| !p.is_empty()).fold(PathBuf::from(&root.path), |acc, p| acc.join(p)))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerStateDto {
    pub id: Option<String>,
    pub playing: bool,
    pub pos: f64,
    pub dur: f64,
    pub shuffle: bool,
    pub repeat: String,
    pub volume: u8,
}

#[derive(Debug, Clone, Serialize)]
struct PosEvent {
    pos: f64,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorEvent {
    id: Option<String>,
    reason: &'static str,
}

/// Saved modes: (shuffle, repeat, volume 0..100). Defaults match the web version.
pub fn load_settings(db: &Db) -> (bool, Repeat, u8) {
    let get = |k: &str| db.setting(k).ok().flatten();
    let shuffle = get("shuffle").is_some_and(|v| v == "true");
    let repeat = get("repeat").and_then(|v| Repeat::parse(&v)).unwrap_or(Repeat::Off);
    let volume = get("volume").and_then(|v| v.parse::<u8>().ok()).map(|v| v.min(100)).unwrap_or(100);
    (shuffle, repeat, volume)
}

pub struct Player {
    engine: OnceLock<Engine>,
    inner: Mutex<Inner>,
}

struct Inner {
    queue: Queue,
    id: Option<String>,
    playing: bool,
    /// the engine holds the current song (false after it ended, failed or stopped)
    loaded: bool,
    pos: f64,
    dur: f64,
    volume: u8,
    failures: usize,
    /// bumped by every user action / load; a pending error-skip only fires if unchanged
    gen: u64,
    /// engine epoch of the track we expect events from (from the last load/seek/stop)
    epoch: u64,
}

impl Inner {
    fn new(queue: Queue, volume: u8) -> Inner {
        Inner { queue, id: None, playing: false, loaded: false, pos: 0.0, dur: 0.0, volume, failures: 0, gen: 0, epoch: 0 }
    }
}

impl Player {
    pub fn new(db: &Db) -> Player {
        let (shuffle, repeat, volume) = load_settings(db);
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let mut queue = Queue::new(seed);
        queue.set_shuffle(shuffle);
        queue.set_repeat(repeat);
        Player {
            engine: OnceLock::new(),
            inner: Mutex::new(Inner::new(queue, volume)),
        }
    }
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn with_engine(&self, f: impl FnOnce(&Engine)) {
        if let Some(e) = self.engine.get() {
            f(e);
        }
    }
    /// Runs an epoch-changing engine call (load/seek/stop) and remembers the new epoch.
    fn retarget(&self, i: &mut Inner, f: impl FnOnce(&Engine) -> u64) {
        if let Some(e) = self.engine.get() {
            i.epoch = f(e);
        }
    }
}

/// Starts the audio engine; call once after `AppState` is managed.
pub fn init(app: &AppHandle) {
    let state = app.state::<AppState>();
    let h = app.clone();
    let engine = Engine::spawn(Box::new(move |epoch, ev| handle_event(&h, epoch, ev)));
    engine.set_volume(state.player.inner().volume as f32 / 100.0);
    let _ = state.player.engine.set(engine);
}

fn dto(i: &Inner) -> PlayerStateDto {
    PlayerStateDto {
        id: i.id.clone(),
        playing: i.playing,
        pos: i.pos,
        dur: i.dur,
        shuffle: i.queue.shuffle(),
        repeat: i.queue.repeat().as_str().to_string(),
        volume: i.volume,
    }
}

fn emit_state(app: &AppHandle, i: &Inner) {
    let _ = app.emit("player://state", dto(i));
}

fn save(state: &AppState, key: &str, value: &str) {
    if let Err(e) = state.db().set_setting(key, value) {
        eprintln!("linger: cannot save setting {key}: {e}");
    }
}

/// Loads the queue's current song into the engine. Lock order: player inner → db.
fn load_current(app: &AppHandle, state: &AppState, i: &mut Inner, autoplay: bool) {
    i.gen += 1;
    i.pos = 0.0;
    i.id = i.queue.current().map(str::to_string);
    let Some(id) = i.id.clone() else {
        i.playing = false;
        i.loaded = false;
        state.player.retarget(i, |e| e.stop());
        return;
    };
    let found = {
        let db = state.db();
        match (db.song(&id), db.roots()) {
            (Ok(Some(song)), Ok(roots)) => song_path(&roots, &song).map(|p| (p, song)),
            _ => None,
        }
    };
    let Some((path, song)) = found else {
        i.dur = 0.0;
        fail(app, i, DecodeError::NotFound);
        return;
    };
    i.dur = song.duration;
    i.playing = autoplay;
    i.loaded = true;
    state.player.retarget(i, |e| e.load(path, song.start_ms, song.end_ms, autoplay));
}

/// A song could not be played: report it, then skip after 1.2 s or stop.
fn fail(app: &AppHandle, i: &mut Inner, e: DecodeError) {
    eprintln!("linger: cannot play {:?}: {e}", i.id);
    let _ = app.emit("player://error", ErrorEvent { id: i.id.clone(), reason: error_reason(&e) });
    i.loaded = false;
    i.failures += 1;
    match on_error(i.failures, i.queue.len()) {
        ErrorAction::Stop => {
            i.playing = false;
            i.failures = 0;
        }
        ErrorAction::SkipAfter(d) => {
            let gen = i.gen;
            let app = app.clone();
            let _ = std::thread::Builder::new().name("linger-skip".into()).spawn(move || {
                std::thread::sleep(d);
                let state = app.state::<AppState>();
                let mut i = state.player.inner();
                if i.gen != gen {
                    return; // the user did something else meanwhile
                }
                // under repeat-one a broken song must not retry itself
                let auto = i.queue.repeat() != Repeat::One;
                if i.queue.next(auto).is_some() {
                    load_current(&app, &state, &mut i, true);
                } else {
                    i.playing = false;
                }
                emit_state(&app, &i);
            });
        }
    }
}

/// What `handle_event` must do after the state update in `apply_event`.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    /// stale event (an older track, or from before a seek): nothing changed
    Ignore,
    Pos(f64),
    /// the queue advanced: load its current song
    Load,
    /// the queue ran out
    Stopped,
    Fail(DecodeError),
    NoOutput,
}

/// The state part of event handling, run under the `inner` lock. Events whose epoch is not
/// the one of our last load/seek/stop are dropped here: an engine thread may emit an event
/// for track A just before a user `next` (holding the lock) loads B.
fn apply_event(i: &mut Inner, epoch: u64, ev: EngineEvent) -> Effect {
    if epoch != i.epoch {
        return Effect::Ignore;
    }
    match ev {
        EngineEvent::Position(pos) => {
            i.pos = pos;
            i.failures = 0; // audio is actually coming out
            Effect::Pos(pos)
        }
        EngineEvent::Ended => {
            i.failures = 0;
            if i.queue.next(true).is_some() {
                Effect::Load
            } else {
                // end of the queue: keep the last song shown, stopped at its start
                i.playing = false;
                i.loaded = false;
                i.pos = 0.0;
                Effect::Stopped
            }
        }
        EngineEvent::Error(e) => Effect::Fail(e),
        EngineEvent::NoOutput => {
            i.playing = false;
            i.loaded = false;
            Effect::NoOutput
        }
    }
}

fn handle_event(app: &AppHandle, epoch: u64, ev: EngineEvent) {
    let state = app.state::<AppState>();
    let mut i = state.player.inner();
    match apply_event(&mut i, epoch, ev) {
        Effect::Ignore => return,
        Effect::Pos(pos) => {
            let _ = app.emit("player://pos", PosEvent { pos });
            return;
        }
        Effect::Load => load_current(app, &state, &mut i, true),
        Effect::Stopped => {}
        Effect::Fail(e) => fail(app, &mut i, e),
        Effect::NoOutput => {
            let _ = app.emit("player://error", ErrorEvent { id: i.id.clone(), reason: NO_OUTPUT });
        }
    }
    emit_state(app, &i);
}

#[tauri::command]
pub fn player_play_list(ids: Vec<String>, start: usize, shuffle: bool, app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    if ids.is_empty() {
        return;
    }
    let was = i.queue.shuffle();
    i.queue.play_list(ids, start, shuffle);
    if i.queue.shuffle() != was {
        save(&state, "shuffle", "true");
    }
    i.failures = 0;
    load_current(&app, &state, &mut i, true);
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_toggle(app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    if i.id.is_none() {
        return;
    }
    if !i.loaded {
        i.failures = 0;
        load_current(&app, &state, &mut i, true);
    } else if i.playing {
        i.playing = false;
        state.player.with_engine(|e| e.pause());
    } else {
        i.playing = true;
        state.player.with_engine(|e| e.play());
    }
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_next(app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    if i.queue.next(false).is_none() {
        return;
    }
    i.failures = 0;
    load_current(&app, &state, &mut i, true);
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_prev(app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    if i.loaded && i.pos > PREV_RESTART_SECS {
        i.gen += 1;
        i.pos = 0.0;
        state.player.retarget(&mut i, |e| e.seek(0.0));
    } else {
        if i.queue.prev().is_none() {
            return;
        }
        i.failures = 0;
        load_current(&app, &state, &mut i, true);
    }
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_seek(secs: f64, app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    if i.id.is_none() || !secs.is_finite() {
        return;
    }
    let secs = if i.dur > 0.0 { secs.clamp(0.0, i.dur) } else { secs.max(0.0) };
    if !i.loaded {
        // ended/stopped: reload paused at the new spot
        load_current(&app, &state, &mut i, false);
        if !i.loaded {
            emit_state(&app, &i);
            return;
        }
    }
    i.gen += 1;
    i.pos = secs;
    state.player.retarget(&mut i, |e| e.seek(secs));
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_set_volume(volume: u8, app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    i.volume = volume.min(100);
    let v = i.volume;
    state.player.with_engine(|e| e.set_volume(v as f32 / 100.0));
    save(&state, "volume", &v.to_string());
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_set_shuffle(on: bool, app: AppHandle, state: State<'_, AppState>) {
    let mut i = state.player.inner();
    i.queue.set_shuffle(on);
    save(&state, "shuffle", if on { "true" } else { "false" });
    emit_state(&app, &i);
}

#[tauri::command]
pub fn player_set_repeat(mode: String, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let r = Repeat::parse(&mode).ok_or_else(|| format!("unknown repeat mode: {mode}"))?;
    let mut i = state.player.inner();
    i.queue.set_repeat(r);
    save(&state, "repeat", r.as_str());
    emit_state(&app, &i);
    Ok(())
}

#[tauri::command]
pub fn player_state(state: State<'_, AppState>) -> PlayerStateDto {
    dto(&state.player.inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn all_failing_queue_stops() {
        assert_eq!(on_error(3, 3), ErrorAction::Stop);
        assert_eq!(on_error(1, 3), ErrorAction::SkipAfter(Duration::from_millis(1200)));
        assert_eq!(on_error(2, 3), ErrorAction::SkipAfter(Duration::from_millis(1200)));
        assert_eq!(on_error(1, 1), ErrorAction::Stop);
        assert_eq!(on_error(5, 3), ErrorAction::Stop);
        assert_eq!(on_error(1, 0), ErrorAction::Stop);
    }

    #[test]
    fn reasons_are_i18n_keys() {
        assert_eq!(error_reason(&DecodeError::NotFound), "player.notFound");
        assert_eq!(error_reason(&DecodeError::NeedFfmpeg), "player.needFfmpeg");
        assert_eq!(error_reason(&DecodeError::Corrupt("x".into())), "player.corrupt");
    }

    #[test]
    fn song_path_joins_root_and_rel() {
        let db = Db::open_in_memory().unwrap();
        let root = db.add_root(r"\\nas\music").unwrap();
        let mut s = test_song(root.id, "Kai/a b/x.flac");
        let p = song_path(&[Root { id: 99, path: "/nope".into() }, root.clone()], &s).unwrap();
        assert_eq!(p, Path::new(r"\\nas\music").join("Kai").join("a b").join("x.flac"));
        s.root_id = 12345;
        assert_eq!(song_path(&[root], &s), None);
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(load_settings(&db), (false, Repeat::Off, 100));
        db.set_setting("shuffle", "true").unwrap();
        db.set_setting("repeat", "one").unwrap();
        db.set_setting("volume", "35").unwrap();
        assert_eq!(load_settings(&db), (true, Repeat::One, 35));
        db.set_setting("repeat", "bogus").unwrap();
        db.set_setting("volume", "900").unwrap();
        assert_eq!(load_settings(&db), (true, Repeat::Off, 100));
        let p = Player::new(&db);
        assert!(p.inner().queue.shuffle());
    }

    fn inner_with(n: usize, epoch: u64) -> Inner {
        let mut q = Queue::new(1);
        q.play_list((0..n).map(|k| format!("s{k}")).collect(), 0, false);
        let mut i = Inner::new(q, 100);
        i.id = Some("s0".into());
        i.playing = true;
        i.loaded = true;
        i.epoch = epoch;
        i
    }

    #[test]
    fn stale_events_are_ignored() {
        // track A (epoch 1) ended, but the user's `next` already loaded B (epoch 2)
        let mut i = inner_with(3, 1);
        i.queue.next(false);
        i.epoch = 2;
        i.pos = 1.5;
        i.failures = 2;
        assert_eq!(apply_event(&mut i, 1, EngineEvent::Ended), Effect::Ignore);
        assert_eq!(i.queue.current(), Some("s1"), "queue must not advance past the user's pick");
        assert_eq!(apply_event(&mut i, 1, EngineEvent::Error(DecodeError::NotFound)), Effect::Ignore);
        assert_eq!(apply_event(&mut i, 1, EngineEvent::Position(99.0)), Effect::Ignore);
        assert_eq!(apply_event(&mut i, 1, EngineEvent::NoOutput), Effect::Ignore);
        assert_eq!((i.pos, i.failures, i.playing, i.loaded), (1.5, 2, true, true));
        // events of the current track still apply
        assert_eq!(apply_event(&mut i, 2, EngineEvent::Position(3.0)), Effect::Pos(3.0));
        assert_eq!((i.pos, i.failures), (3.0, 0));
        assert_eq!(apply_event(&mut i, 2, EngineEvent::Ended), Effect::Load);
        assert_eq!(i.queue.current(), Some("s2"));
        assert_eq!(apply_event(&mut i, 2, EngineEvent::Error(DecodeError::NotFound)), Effect::Fail(DecodeError::NotFound));
    }

    #[test]
    fn ended_at_queue_end_stops() {
        let mut i = inner_with(1, 4);
        assert_eq!(apply_event(&mut i, 4, EngineEvent::Ended), Effect::Stopped);
        assert_eq!((i.playing, i.loaded, i.pos, i.queue.current()), (false, false, 0.0, Some("s0")));
        let mut i = inner_with(1, 4);
        assert_eq!(apply_event(&mut i, 4, EngineEvent::NoOutput), Effect::NoOutput);
        assert!(!i.playing && !i.loaded);
    }

    fn test_song(root_id: i64, rel: &str) -> SongRow {
        SongRow { id: "x".into(), root_id, file_id: 1, rel: rel.into(), ext: "flac".into(), codec: "flac".into(),
            bitrate: 0, size: 0, cover: 0, title: String::new(), artist: String::new(), album: String::new(),
            track: 0, start_ms: 0, end_ms: None, duration: 0.0 }
    }
}
