//! Volume balance: measures each song's loudness (EBU R128) in the background and keeps the
//! result in Linger's database. Music files are only read.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ebur128::{EbuR128, Mode};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::SongRow;
use crate::player::decode::{find_ffmpeg, open_decoder, DecodeError};
use crate::state::AppState;

const ENABLED_KEY: &str = "normalize_volume";
pub const TARGET_LUFS: f64 = -14.0;
const MAX_BOOST_DB: f64 = 6.0;
const MAX_CUT_DB: f64 = 18.0;
/// Boosting stops before a song's loudest sample reaches this level.
const PEAK_CEILING: f64 = 0.98;
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
const BETWEEN_FILES: Duration = Duration::from_millis(20);
const SCAN_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    pub lufs: f64,
    /// highest sample, 1.0 = full scale
    pub peak: f64,
}

/// A song waiting to be measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub root_id: i64,
    pub root_path: String,
    pub rel: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub size: u64,
    pub mtime: i64,
}

pub struct Loudness {
    wake: mpsc::Sender<()>,
    enabled: AtomicBool,
    /// a measuring pass is under way
    running: AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SoundSettings {
    pub normalize: bool,
    pub running: bool,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    running: bool,
    done: usize,
    total: usize,
}

/// Linear gain that brings a song to the target without clipping it.
pub fn gain_for(m: Measure) -> f32 {
    if !m.lufs.is_finite() {
        return 1.0;
    }
    let db = (TARGET_LUFS - m.lufs).clamp(-MAX_CUT_DB, MAX_BOOST_DB);
    let linear = 10f64.powf(db / 20.0);
    if linear <= 1.0 {
        return linear as f32;
    }
    let room = if m.peak > 0.0 { (PEAK_CEILING / m.peak).max(1.0) } else { linear };
    linear.min(room) as f32
}

/// One measure for a whole album: the average energy, and its loudest sample.
pub fn combine(all: &[Measure]) -> Option<Measure> {
    if all.is_empty() {
        return None;
    }
    let energy = all.iter().map(|m| 10f64.powf(m.lufs / 10.0)).sum::<f64>() / all.len() as f64;
    let peak = all.iter().map(|m| m.peak).fold(0.0, f64::max);
    Some(Measure { lufs: 10.0 * energy.log10(), peak })
}

fn folder(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

fn same_album(a: &SongRow, b: &SongRow) -> bool {
    !a.album.trim().is_empty() && a.root_id == b.root_id && a.album == b.album && folder(&a.rel) == folder(&b.rel)
}

fn job_path(job: &Job) -> PathBuf {
    job.rel.split('/').filter(|p| !p.is_empty()).fold(PathBuf::from(&job.root_path), |p, part| p.join(part))
}

/// Reads the whole song. `Ok(None)`: it has no measurable sound.
pub fn measure(
    path: &Path,
    start_ms: u64,
    end_ms: Option<u64>,
    ffmpeg: Option<&Path>,
    keep_going: &dyn Fn() -> bool,
) -> Result<Option<Measure>, DecodeError> {
    let mut dec = open_decoder(path, start_ms, end_ms, ffmpeg)?;
    let channels = dec.channels().max(1) as usize;
    let mut meter = EbuR128::new(channels as u32, dec.sample_rate(), Mode::I | Mode::SAMPLE_PEAK)
        .map_err(|_| DecodeError::Unsupported)?;
    while let Some(chunk) = dec.next_chunk()? {
        if !keep_going() {
            return Err(DecodeError::Io("stopped".into()));
        }
        let whole = chunk.len() - chunk.len() % channels;
        meter.add_frames_f32(&chunk[..whole]).map_err(|_| DecodeError::Unsupported)?;
    }
    let Ok(lufs) = meter.loudness_global() else { return Ok(None) };
    if !lufs.is_finite() {
        return Ok(None);
    }
    let mut peak = 0.0f64;
    for c in 0..channels {
        peak = peak.max(meter.sample_peak(c as u32).unwrap_or(0.0));
    }
    Ok(Some(Measure { lufs, peak }))
}

/// The gain for a song about to play. Album neighbours in the queue share one gain.
pub fn song_gain(app: &AppHandle, song: &SongRow, neighbours: &[String]) -> f32 {
    let Some(l) = app.try_state::<Loudness>() else { return 1.0 };
    if !l.enabled.load(Ordering::SeqCst) {
        return 1.0;
    }
    let state = app.state::<AppState>();
    let db = state.db();
    let Some(own) = db.loudness_of(&song.id).ok().flatten() else { return 1.0 };
    let in_album = neighbours.iter().any(|id| db.song(id).ok().flatten().is_some_and(|n| same_album(song, &n)));
    if in_album {
        let tracks: Vec<Measure> = db.all_loudness_for_album(song.root_id, folder(&song.rel), &song.album).unwrap_or_default();
        if let Some(album) = combine(&tracks) {
            return gain_for(album);
        }
    }
    gain_for(own)
}

fn publish(app: &AppHandle, l: &Loudness) {
    let state = app.state::<AppState>();
    let (done, total) = state.db().loudness_counts().unwrap_or((0, 0));
    let _ = app.emit("loudness://progress", Progress { running: l.running.load(Ordering::SeqCst), done, total });
}

fn pass(app: &AppHandle) {
    let state = app.state::<AppState>();
    let l = app.state::<Loudness>();
    let jobs = {
        let db = state.db();
        let _ = db.prune_loudness();
        db.loudness_jobs().unwrap_or_default()
    };
    publish(app, &l);
    let current = state.player.current_id();
    let (first, rest): (Vec<Job>, Vec<Job>) = jobs.into_iter().partition(|j| Some(&j.id) == current.as_ref());
    let ffmpeg = find_ffmpeg();
    let mut shown = Instant::now();
    let keep_going = || l.enabled.load(Ordering::SeqCst);
    for job in first.into_iter().chain(rest) {
        while keep_going() && state.scan.is_running() {
            std::thread::sleep(SCAN_WAIT);
        }
        if !keep_going() {
            break;
        }
        if !state.is_online(job.root_id) {
            continue;
        }
        let result = measure(&job_path(&job), job.start_ms, job.end_ms, ffmpeg.as_deref(), &keep_going);
        let value = match result {
            Ok(m) => Some(m),
            // a broken file is not tried again until it changes
            Err(DecodeError::Corrupt(_) | DecodeError::Unsupported) => Some(None),
            // missing FFmpeg, a dropped share or a stop: try again on a later pass
            Err(_) => None,
        };
        if let Some(m) = value {
            let _ = state.db().put_loudness(&job.id, job.size, job.mtime, m);
        }
        if shown.elapsed() >= PROGRESS_EVERY {
            shown = Instant::now();
            publish(app, &l);
        }
        std::thread::sleep(BETWEEN_FILES);
    }
    publish(app, &l);
}

fn worker(app: AppHandle, rx: mpsc::Receiver<()>) {
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        let l = app.state::<Loudness>();
        if l.enabled.load(Ordering::SeqCst) {
            l.running.store(true, Ordering::SeqCst);
            pass(&app);
            l.running.store(false, Ordering::SeqCst);
            publish(&app, &l);
        }
    }
}

pub fn init(app: &AppHandle) {
    let state = app.state::<AppState>();
    let enabled = state.db().setting(ENABLED_KEY).ok().flatten().as_deref() == Some("true");
    let (tx, rx) = mpsc::channel();
    app.manage(Loudness { wake: tx, enabled: AtomicBool::new(enabled), running: AtomicBool::new(false) });
    let h = app.clone();
    if let Err(e) = std::thread::Builder::new().name("linger-loudness".into()).spawn(move || worker(h, rx)) {
        eprintln!("linger: cannot start the volume analysis: {e}");
    }
    if enabled {
        wake(app);
    }
}

/// New or changed songs may need measuring (a scan finished).
pub fn wake(app: &AppHandle) {
    if let Some(l) = app.try_state::<Loudness>() {
        let _ = l.wake.send(());
    }
}

#[tauri::command(async)]
pub fn sound_settings(app: AppHandle) -> SoundSettings {
    let state = app.state::<AppState>();
    let (done, total) = state.db().loudness_counts().unwrap_or((0, 0));
    let l = app.try_state::<Loudness>();
    let normalize = l.as_ref().is_some_and(|l| l.enabled.load(Ordering::SeqCst));
    let running = l.as_ref().is_some_and(|l| l.running.load(Ordering::SeqCst));
    SoundSettings { normalize, running, done, total }
}

#[tauri::command(async)]
pub fn sound_set_normalize(enabled: bool, app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let saved = state.db().set_setting(ENABLED_KEY, if enabled { "true" } else { "false" });
        saved.map_err(|e| e.to_string())?;
    }
    if let Some(l) = app.try_state::<Loudness>() {
        l.enabled.store(enabled, Ordering::SeqCst);
        if enabled {
            let _ = l.wake.send(());
        }
    }
    crate::commands::player::refresh_gain(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(lufs: f64, peak: f64) -> Measure {
        Measure { lufs, peak }
    }

    #[test]
    fn loud_songs_are_turned_down() {
        assert!((gain_for(m(-10.0, 1.0)) - 10f32.powf(-4.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn quiet_songs_are_boosted_only_while_they_do_not_clip() {
        assert!((gain_for(m(-22.0, 0.2)) - 10f32.powf(6.0 / 20.0)).abs() < 1e-4);
        assert!((gain_for(m(-22.0, 0.7)) - (0.98 / 0.7) as f32).abs() < 1e-4);
        assert_eq!(gain_for(m(-22.0, 1.0)), 1.0);
    }

    #[test]
    fn extreme_values_are_limited() {
        assert!((gain_for(m(-60.0, 0.01)) - 10f32.powf(6.0 / 20.0)).abs() < 1e-4);
        assert!((gain_for(m(10.0, 1.0)) - 10f32.powf(-18.0 / 20.0)).abs() < 1e-4);
        assert_eq!(gain_for(m(f64::NEG_INFINITY, 0.0)), 1.0);
        assert_eq!(gain_for(m(f64::NAN, 0.5)), 1.0);
    }

    #[test]
    fn an_album_averages_energy_and_keeps_the_top_peak() {
        let a = combine(&[m(-10.0, 0.9), m(-20.0, 0.5)]).unwrap();
        assert!((a.lufs - (-12.6)).abs() < 0.05);
        assert_eq!(a.peak, 0.9);
        assert_eq!(combine(&[]), None);
    }

    #[test]
    fn measures_a_real_decoded_file() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("tone.wav");
        std::fs::write(&p, crate::tags::testutil::wav_tone(44_100, 2, 3.0, 0.25)).unwrap();
        let r = measure(&p, 0, None, None, &|| true).unwrap().unwrap();
        assert!(r.lufs > -20.0 && r.lufs < -8.0, "{}", r.lufs);
        assert!((r.peak - 0.25).abs() < 0.02, "{}", r.peak);
    }

    #[test]
    fn measuring_can_be_stopped() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("tone.wav");
        std::fs::write(&p, crate::tags::testutil::wav_tone(44_100, 2, 3.0, 0.25)).unwrap();
        assert!(measure(&p, 0, None, None, &|| false).is_err());
    }
}
