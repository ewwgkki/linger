//! Output engine. rodio's `OutputStream` is not `Send`, so it lives on the engine thread,
//! controlled through a command channel. Each load/seek spawns a decode thread that fills a
//! bounded channel (~4 s of audio); the rodio source drains it and outputs silence when it
//! runs dry (network hiccups), ending only on the decoder's explicit end marker.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use super::decode::{find_ffmpeg, open_decoder, DecodeError};

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// seconds from the start of the song (segment)
    Position(f64),
    Ended,
    Error(DecodeError),
    /// no audio output device could be opened
    NoOutput,
}

enum Cmd {
    Load { path: PathBuf, start_ms: u64, end_ms: Option<u64>, autoplay: bool, gain: f32, epoch: u64 },
    Play,
    Pause,
    Seek(f64, u64),
    Volume(f32),
    Gain(f32),
    Stop,
}

pub struct Engine {
    tx: mpsc::Sender<Cmd>,
    /// bumped by every load/seek/stop. The engine thread drops events of an older epoch,
    /// and every event carries its epoch so the receiver can re-check under its own lock
    /// (an event already emitted may still race a newer `load`).
    epoch: Arc<AtomicU64>,
}

const TICK: Duration = Duration::from_millis(50);
const POS_EVERY: Duration = Duration::from_millis(250);
/// Each chunk in the prefetch channel holds ~100 ms; 40 of them ≈ 4 s.
const CHUNKS_PER_SEC: usize = 10;
const PREFETCH_CHUNKS: usize = 40;
/// Short fade on pause, resume, skip and seek, so sound never cuts with a click.
const FADE: Duration = Duration::from_millis(180);

impl Engine {
    /// `on_event(epoch, event)`: `epoch` is the value returned by the `load`/`seek` that
    /// started the track the event belongs to (0 for engine-level events at startup).
    pub fn spawn(on_event: Box<dyn Fn(u64, EngineEvent) + Send>) -> Engine {
        let (tx, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let ep = epoch.clone();
        let spawned = std::thread::Builder::new().name("linger-audio".into()).spawn(move || run(rx, ep, on_event));
        if let Err(e) = spawned {
            eprintln!("linger: cannot start audio thread: {e}");
        }
        Engine { tx, epoch }
    }
    /// Returns the new epoch; events for this track carry it.
    pub fn load(&self, path: PathBuf, start_ms: u64, end_ms: Option<u64>, autoplay: bool, gain: f32) -> u64 {
        let epoch = self.bump();
        self.send(Cmd::Load { path, start_ms, end_ms, autoplay, gain, epoch });
        epoch
    }
    pub fn play(&self) {
        self.send(Cmd::Play);
    }
    pub fn pause(&self) {
        self.send(Cmd::Pause);
    }
    /// `secs` from the start of the song (segment).
    /// Returns the new epoch (positions from before the seek are stale).
    pub fn seek(&self, secs: f64) -> u64 {
        let epoch = self.bump();
        self.send(Cmd::Seek(secs, epoch));
        epoch
    }
    /// 0..1 (applied as v², which sounds more linear)
    pub fn set_volume(&self, v: f32) {
        self.send(Cmd::Volume(v));
    }
    /// Volume balance for the current song (1.0 = unchanged).
    pub fn set_gain(&self, gain: f32) {
        self.send(Cmd::Gain(gain));
    }
    /// Returns the new epoch.
    pub fn stop(&self) -> u64 {
        let epoch = self.bump();
        self.send(Cmd::Stop);
        epoch
    }
    fn bump(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn send(&self, c: Cmd) {
        // a dead engine thread must not take the app down
        let _ = self.tx.send(c);
    }
}

// ---------- decode thread ----------

enum Msg {
    Data(Vec<f32>),
    End,
    Error(DecodeError),
}

type Header = Result<(u32, u16), DecodeError>;

fn spawn_decoder(path: PathBuf, start_ms: u64, end_ms: Option<u64>) -> (Receiver<Header>, Receiver<Msg>) {
    let (hdr_tx, hdr_rx) = mpsc::sync_channel::<Header>(1);
    let (data_tx, data_rx) = mpsc::sync_channel::<Msg>(PREFETCH_CHUNKS);
    let spawned = std::thread::Builder::new()
        .name("linger-decode".into())
        .spawn(move || decode_thread(path, start_ms, end_ms, hdr_tx, data_tx));
    if let Err(e) = spawned {
        // the dropped senders surface as a disconnect, which the engine reports
        eprintln!("linger: cannot start decode thread: {e}");
    }
    (hdr_rx, data_rx)
}

/// Exits as soon as the engine drops its receivers (send fails); dropping the decoder
/// kills an FFmpeg child.
fn decode_thread(path: PathBuf, start_ms: u64, end_ms: Option<u64>, hdr_tx: SyncSender<Header>, data_tx: SyncSender<Msg>) {
    let ff = find_ffmpeg();
    let mut dec = match open_decoder(&path, start_ms, end_ms, ff.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            let _ = hdr_tx.send(Err(e));
            return;
        }
    };
    if hdr_tx.send(Ok((dec.sample_rate(), dec.channels()))).is_err() {
        return;
    }
    let target = (dec.sample_rate() as usize * dec.channels() as usize / CHUNKS_PER_SEC).max(1);
    let mut acc: Vec<f32> = Vec::with_capacity(target * 2);
    loop {
        let last = match dec.next_chunk() {
            Ok(Some(c)) => {
                acc.extend_from_slice(&c);
                if acc.len() >= target {
                    let full = std::mem::replace(&mut acc, Vec::with_capacity(target * 2));
                    if data_tx.send(Msg::Data(full)).is_err() {
                        return;
                    }
                }
                continue;
            }
            Ok(None) => Msg::End,
            Err(e) => Msg::Error(e),
        };
        if !acc.is_empty() && data_tx.send(Msg::Data(acc)).is_err() {
            return;
        }
        let _ = data_tx.send(last);
        return;
    }
}

// ---------- rodio source ----------

#[derive(Default)]
struct Shared {
    /// samples handed to the output
    played: AtomicU64,
    ended: AtomicBool,
    error: Mutex<Option<DecodeError>>,
}

/// Moves a level toward its target by a fixed step per sample.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ramp {
    value: f32,
    step: f32,
}

impl Ramp {
    fn new(value: f32, rate: u32, channels: u16) -> Ramp {
        let samples = (rate as f32 * channels.max(1) as f32 * FADE.as_secs_f32()).max(1.0);
        Ramp { value, step: 1.0 / samples }
    }
    fn next(&mut self, target: f32) -> f32 {
        self.value = if self.value < target { (self.value + self.step).min(target) } else { (self.value - self.step).max(target) };
        self.value
    }
}

/// Live targets the audio callback reads without locking.
struct Controls {
    level: AtomicU32,
    gain: AtomicU32,
}

impl Controls {
    fn new(level: f32, gain: f32) -> Arc<Controls> {
        Arc::new(Controls { level: AtomicU32::new(level.to_bits()), gain: AtomicU32::new(gain.to_bits()) })
    }
    fn fade_to(&self, level: f32) {
        self.level.store(level.to_bits(), Ordering::Relaxed);
    }
    fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }
    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

struct StreamSource {
    controls: Arc<Controls>,
    level: Ramp,
    gain: Ramp,
    rx: Receiver<Msg>,
    cur: Vec<f32>,
    idx: usize,
    rate: u32,
    channels: u16,
    shared: Arc<Shared>,
    played: u64,
    /// remaining samples of the current silent frame (keeps channels aligned)
    silence: u16,
    finished: bool,
}

impl StreamSource {
    fn finish(&mut self, err: Option<DecodeError>) -> Option<f32> {
        self.shared.played.store(self.played, Ordering::Relaxed);
        match err {
            Some(e) => *self.shared.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e),
            None => self.shared.ended.store(true, Ordering::SeqCst),
        }
        self.finished = true;
        None
    }
}

impl Iterator for StreamSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.finished {
            return None;
        }
        if self.silence > 0 {
            self.silence -= 1;
            return Some(0.0);
        }
        loop {
            if let Some(&s) = self.cur.get(self.idx) {
                self.idx += 1;
                self.played += 1;
                if self.played.is_multiple_of(2048) {
                    self.shared.played.store(self.played, Ordering::Relaxed);
                }
                let level = self.level.next(self.controls.level());
                let gain = self.gain.next(self.controls.gain());
                return Some((s * level * gain).clamp(-1.0, 1.0));
            }
            self.shared.played.store(self.played, Ordering::Relaxed);
            match self.rx.try_recv() {
                Ok(Msg::Data(v)) => {
                    self.cur = v;
                    self.idx = 0;
                }
                Ok(Msg::End) => return self.finish(None),
                Ok(Msg::Error(e)) => return self.finish(Some(e)),
                Err(TryRecvError::Empty) => {
                    // prefetch ran dry: play one frame of silence instead of ending
                    self.silence = self.channels.saturating_sub(1);
                    return Some(0.0);
                }
                Err(TryRecvError::Disconnected) => {
                    return self.finish(Some(DecodeError::Io("decoder stopped unexpectedly".into())))
                }
            }
        }
    }
}

impl Source for StreamSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

// ---------- engine thread ----------

struct Track {
    epoch: u64,
    path: PathBuf,
    start_ms: u64,
    end_ms: Option<u64>,
    /// seconds into the song where the current decoder started
    offset: f64,
    playing: bool,
    /// waiting for the decoder to open: (header, data)
    pending: Option<(Receiver<Header>, Receiver<Msg>)>,
    sink: Option<Sink>,
    shared: Arc<Shared>,
    /// samples per second (rate × channels) of the current decoder
    frame_samples: f64,
    controls: Arc<Controls>,
    gain: f32,
    /// replaces sound that was audible, so it fades in
    fade_in: bool,
    /// the sink pauses once the fade-out has finished
    pause_at: Option<Instant>,
}

impl Track {
    fn position(&self) -> f64 {
        let played = self.shared.played.load(Ordering::Relaxed) as f64;
        if self.frame_samples > 0.0 {
            self.offset + played / self.frame_samples
        } else {
            self.offset
        }
    }
    fn restart(&mut self, secs: f64, fading: &mut Vec<(Sink, Instant)>) {
        // the old source fades out; when its sink drops, the decode thread exits
        if retire(self.sink.take(), &self.controls, fading) {
            self.fade_in = true;
        }
        self.pause_at = None;
        self.controls = Controls::new(if self.playing { 1.0 } else { 0.0 }, self.gain);
        self.shared = Arc::default();
        self.frame_samples = 0.0;
        self.offset = secs;
        let at = self.start_ms + (secs * 1000.0) as u64;
        self.pending = Some(spawn_decoder(self.path.clone(), at, self.end_ms));
    }
}

/// Lets a replaced sink fade out. Returns whether it was audible.
fn retire(sink: Option<Sink>, controls: &Controls, fading: &mut Vec<(Sink, Instant)>) -> bool {
    let Some(sink) = sink else { return false };
    if sink.is_paused() {
        return false;
    }
    controls.fade_to(0.0);
    fading.push((sink, Instant::now() + FADE + TICK));
    true
}

fn open_output() -> Option<(OutputStream, OutputStreamHandle)> {
    match OutputStream::try_default() {
        Ok(o) => Some(o),
        Err(e) => {
            eprintln!("linger: no audio output: {e}");
            None
        }
    }
}

fn run(rx: Receiver<Cmd>, epoch: Arc<AtomicU64>, on_event: Box<dyn Fn(u64, EngineEvent) + Send>) {
    let emit = |ep: u64, e| on_event(ep, e);
    let current = |t: &Track| t.epoch == epoch.load(Ordering::SeqCst);
    let mut output = open_output();
    if output.is_none() {
        emit(0, EngineEvent::NoOutput);
    }
    let mut volume: f32 = 1.0;
    let mut track: Option<Track> = None;
    let mut last_pos = Instant::now();
    let mut fading: Vec<(Sink, Instant)> = Vec::new();

    loop {
        let received = rx.recv_timeout(TICK);
        let now = Instant::now();
        fading.retain(|(_, until)| *until > now);
        match received {
            Ok(Cmd::Load { path, start_ms, end_ms, autoplay, gain, epoch: ep }) => {
                let replaced = track.take().is_some_and(|mut old| retire(old.sink.take(), &old.controls, &mut fading));
                if output.is_none() {
                    output = open_output(); // a device may have been plugged in since
                }
                if output.is_none() {
                    emit(ep, EngineEvent::NoOutput);
                    continue;
                }
                let mut t = Track {
                    epoch: ep,
                    path,
                    start_ms,
                    end_ms,
                    offset: 0.0,
                    playing: autoplay,
                    pending: None,
                    sink: None,
                    shared: Arc::default(),
                    frame_samples: 0.0,
                    controls: Controls::new(if autoplay { 1.0 } else { 0.0 }, gain),
                    gain,
                    fade_in: replaced,
                    pause_at: None,
                };
                t.restart(0.0, &mut fading);
                track = Some(t);
            }
            Ok(Cmd::Play) => {
                if let Some(t) = track.as_mut() {
                    t.playing = true;
                    t.pause_at = None;
                    t.controls.fade_to(1.0);
                    match &t.sink {
                        Some(s) => s.play(),
                        // reopened partway through a song: it should not start abruptly
                        None => t.fade_in = true,
                    }
                }
            }
            Ok(Cmd::Pause) => {
                if let Some(t) = track.as_mut() {
                    t.playing = false;
                    t.controls.fade_to(0.0);
                    if t.sink.as_ref().is_some_and(|s| !s.is_paused()) {
                        t.pause_at = Some(Instant::now() + FADE);
                    }
                }
            }
            Ok(Cmd::Seek(secs, ep)) => {
                if let Some(t) = track.as_mut() {
                    t.epoch = ep;
                    t.restart(if secs.is_finite() { secs.max(0.0) } else { 0.0 }, &mut fading);
                }
            }
            Ok(Cmd::Volume(v)) => {
                volume = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
                if let Some(s) = track.as_ref().and_then(|t| t.sink.as_ref()) {
                    s.set_volume(volume * volume);
                }
            }
            Ok(Cmd::Gain(g)) => {
                if let Some(t) = track.as_mut() {
                    t.gain = g;
                    t.controls.set_gain(g);
                }
            }
            Ok(Cmd::Stop) => {
                if let Some(mut old) = track.take() {
                    retire(old.sink.take(), &old.controls, &mut fading);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let Some(t) = track.as_mut() else { continue };
        if t.pause_at.is_some_and(|at| Instant::now() >= at) {
            t.pause_at = None;
            if let Some(s) = &t.sink {
                s.pause();
            }
        }
        if !current(t) {
            continue; // superseded by a queued load/seek/stop
        }
        let ep = t.epoch;

        // decoder opened?
        if let Some((hdr, _)) = &t.pending {
            let header = match hdr.try_recv() {
                Ok(h) => h,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Err(DecodeError::Io("decoder thread died".into())),
            };
            let (_, data) = t.pending.take().expect("pending");
            let (rate, channels) = match header {
                Ok(h) => h,
                Err(e) => {
                    track = None;
                    emit(ep, EngineEvent::Error(e));
                    continue;
                }
            };
            let sink = match output.as_ref().map(|(_, h)| Sink::try_new(h)) {
                Some(Ok(s)) => s,
                _ => {
                    eprintln!("linger: cannot open an output sink");
                    output = None;
                    track = None;
                    emit(ep, EngineEvent::NoOutput);
                    continue;
                }
            };
            sink.set_volume(volume * volume);
            if !t.playing {
                sink.pause();
            }
            t.frame_samples = rate as f64 * channels as f64;
            let start_level = if t.fade_in || !t.playing { 0.0 } else { 1.0 };
            t.fade_in = false;
            sink.append(StreamSource {
                controls: t.controls.clone(),
                level: Ramp::new(start_level, rate, channels),
                gain: Ramp { value: t.gain, ..Ramp::new(t.gain, rate, channels) },
                rx: data,
                cur: Vec::new(),
                idx: 0,
                rate,
                channels,
                shared: t.shared.clone(),
                played: 0,
                silence: 0,
                finished: false,
            });
            t.sink = Some(sink);
            last_pos = Instant::now();
        }

        // failed mid-stream or finished?
        let err = t.shared.error.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(e) = err {
            track = None;
            emit(ep, EngineEvent::Error(e));
            continue;
        }
        if t.shared.ended.load(Ordering::SeqCst) {
            if t.sink.as_ref().is_some_and(|s| !s.empty()) {
                continue; // the mixer is still draining what it pulled
            }
            let pos = t.position();
            track = None;
            emit(ep, EngineEvent::Position(pos));
            emit(ep, EngineEvent::Ended);
            continue;
        }
        if t.playing && t.sink.is_some() && last_pos.elapsed() >= POS_EVERY {
            last_pos = Instant::now();
            if t.shared.played.load(Ordering::Relaxed) > 0 {
                emit(ep, EngineEvent::Position(t.position()));
            }
        }
    }
}

#[cfg(test)]
mod fade_tests {
    use super::*;

    #[test]
    fn a_fade_reaches_its_target_in_time_and_stops_there() {
        let mut r = Ramp::new(0.0, 44_100, 2);
        let samples = (44_100.0 * 2.0 * FADE.as_secs_f64()).ceil() as usize + 1;
        for _ in 0..samples {
            r.next(1.0);
        }
        assert_eq!(r.value, 1.0);
        assert_eq!(r.next(1.0), 1.0);
        for _ in 0..samples {
            r.next(0.0);
        }
        assert_eq!(r.value, 0.0);
    }

    #[test]
    fn a_fade_never_jumps() {
        let mut r = Ramp::new(0.0, 44_100, 2);
        let mut last = r.value;
        for _ in 0..2_000 {
            let v = r.next(1.0);
            assert!(v >= last && v - last <= r.step + f32::EPSILON);
            last = v;
        }
    }

    fn source(samples: Vec<f32>, level: f32, gain: f32) -> StreamSource {
        let (tx, rx) = mpsc::sync_channel(2);
        tx.send(Msg::Data(samples)).unwrap();
        tx.send(Msg::End).unwrap();
        StreamSource {
            controls: Controls::new(level, gain),
            level: Ramp::new(level, 44_100, 2),
            gain: Ramp::new(gain, 44_100, 2),
            rx,
            cur: Vec::new(),
            idx: 0,
            rate: 44_100,
            channels: 2,
            shared: Arc::default(),
            played: 0,
            silence: 0,
            finished: false,
        }
    }

    #[test]
    fn gain_scales_the_sound() {
        let out: Vec<f32> = source(vec![0.5, -0.5], 1.0, 0.5).collect();
        assert_eq!(out, vec![0.25, -0.25]);
    }

    #[test]
    fn a_boost_cannot_clip() {
        let out: Vec<f32> = source(vec![0.8, -0.8], 1.0, 2.0).collect();
        assert_eq!(out, vec![1.0, -1.0]);
    }

    #[test]
    fn unity_leaves_samples_untouched() {
        let input = vec![0.1, -0.2, 0.3, -0.4];
        let out: Vec<f32> = source(input.clone(), 1.0, 1.0).collect();
        assert_eq!(out, input);
    }
}
