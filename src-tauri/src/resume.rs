//! Remembers the song, position and queue so Linger reopens where it stopped, paused.

use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::db::Db;
use crate::player::queue::QueueSnapshot;
use crate::state::AppState;

const QUEUE_KEY: &str = "resume_queue";
const POS_KEY: &str = "resume_pos";
/// A song this close to its end starts again from the top.
const END_MARGIN: f64 = 3.0;
/// While playing, the position is saved this often.
const SAVE_EVERY: Duration = Duration::from_secs(15);
const IDLE_WAIT: Duration = Duration::from_secs(3600);
/// Quick changes (skipping through songs) are saved once.
const SETTLE: Duration = Duration::from_millis(700);
const MIN_POS_CHANGE: f64 = 1.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPos {
    pub id: String,
    pub pos: f64,
}

/// What the player reports to be saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub queue: QueueSnapshot,
    pub id: Option<String>,
    pub pos: f64,
    pub playing: bool,
}

#[derive(Default)]
struct Written {
    queue: Option<String>,
    pos: Option<SavedPos>,
}

pub struct Resume {
    refresh: mpsc::Sender<()>,
    written: Mutex<Written>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The position to reopen at, in seconds.
pub fn start_pos(saved: f64, duration: f64) -> f64 {
    if !saved.is_finite() || saved <= 0.0 || (duration > 0.0 && saved >= duration - END_MARGIN) {
        0.0
    } else {
        saved
    }
}

fn pos_changed(last: Option<&SavedPos>, id: &str, pos: f64) -> bool {
    last.is_none_or(|l| l.id != id || (l.pos - pos).abs() >= MIN_POS_CHANGE)
}

/// The saved session; damaged or missing data simply means a fresh start.
pub fn read(db: &Db) -> Option<(QueueSnapshot, Option<SavedPos>)> {
    let queue = serde_json::from_str(&db.setting(QUEUE_KEY).ok()??).ok()?;
    let pos = db.setting(POS_KEY).ok().flatten().and_then(|v| serde_json::from_str(&v).ok());
    Some((queue, pos))
}

fn write(state: &AppState, written: &Mutex<Written>, force: bool) {
    let s = state.player.session();
    let Some(id) = s.id else { return };
    let Ok(queue) = serde_json::to_string(&s.queue) else { return };
    let mut last = lock(written);
    let db = state.db();
    if force || last.queue.as_deref() != Some(queue.as_str()) {
        match db.set_setting(QUEUE_KEY, &queue) {
            Ok(()) => last.queue = Some(queue),
            Err(e) => eprintln!("linger: cannot save the queue: {e}"),
        }
    }
    if force || pos_changed(last.pos.as_ref(), &id, s.pos) {
        let saved = SavedPos { id, pos: s.pos };
        if let Ok(text) = serde_json::to_string(&saved) {
            match db.set_setting(POS_KEY, &text) {
                Ok(()) => last.pos = Some(saved),
                Err(e) => eprintln!("linger: cannot save the position: {e}"),
            }
        }
    }
}

fn worker(app: AppHandle, rx: mpsc::Receiver<()>) {
    let mut wait = IDLE_WAIT;
    loop {
        match rx.recv_timeout(wait) {
            Ok(()) => {
                std::thread::sleep(SETTLE);
                while rx.try_recv().is_ok() {}
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let state = app.state::<AppState>();
        let resume = app.state::<Resume>();
        write(&state, &resume.written, false);
        wait = if state.player.session_playing() { SAVE_EVERY } else { IDLE_WAIT };
    }
}

/// Call after the player has restored its session.
pub fn init(app: &AppHandle) {
    let (refresh, rx) = mpsc::channel();
    app.manage(Resume { refresh, written: Mutex::new(Written::default()) });
    let h = app.clone();
    if let Err(e) = std::thread::Builder::new().name("linger-resume".into()).spawn(move || worker(h, rx)) {
        eprintln!("linger: cannot start saving the session: {e}");
    }
}

/// The player or queue changed. Never blocks, so it is safe under the player lock.
pub fn notify(app: &AppHandle) {
    if let Some(r) = app.try_state::<Resume>() {
        let _ = r.refresh.send(());
    }
}

/// Saves right away; call before the app exits.
pub fn save_now(app: &AppHandle) {
    if let Some(r) = app.try_state::<Resume>() {
        write(&app.state::<AppState>(), &r.written, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_keeps_the_position() {
        assert_eq!(start_pos(42.5, 200.0), 42.5);
        assert_eq!(start_pos(42.5, 0.0), 42.5);
    }

    #[test]
    fn a_finished_song_starts_over() {
        assert_eq!(start_pos(198.0, 200.0), 0.0);
        assert_eq!(start_pos(250.0, 200.0), 0.0);
    }

    #[test]
    fn bad_positions_start_over() {
        assert_eq!(start_pos(f64::NAN, 200.0), 0.0);
        assert_eq!(start_pos(-3.0, 200.0), 0.0);
    }

    #[test]
    fn small_position_changes_are_not_written() {
        let last = SavedPos { id: "a".into(), pos: 10.0 };
        assert!(!pos_changed(Some(&last), "a", 10.5));
        assert!(pos_changed(Some(&last), "a", 11.0));
        assert!(pos_changed(Some(&last), "b", 10.0));
        assert!(pos_changed(None, "a", 0.0));
    }

    #[test]
    fn saved_session_round_trips_and_damage_is_ignored() {
        let db = Db::open_in_memory().unwrap();
        assert!(read(&db).is_none());
        let q = QueueSnapshot { up: vec!["u".into()], extra: None, items: vec!["a".into(), "b".into()], order: vec![1, 0], pos: 1 };
        db.set_setting(QUEUE_KEY, &serde_json::to_string(&q).unwrap()).unwrap();
        db.set_setting(POS_KEY, r#"{"id":"a","pos":12.0}"#).unwrap();
        assert_eq!(read(&db), Some((q.clone(), Some(SavedPos { id: "a".into(), pos: 12.0 }))));
        db.set_setting(POS_KEY, "broken").unwrap();
        assert_eq!(read(&db), Some((q, None)));
        db.set_setting(QUEUE_KEY, "broken").unwrap();
        assert!(read(&db).is_none());
    }
}
