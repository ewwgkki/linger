//! Output engine. rodio's `OutputStream` is not `Send`, so it lives on the engine thread,
//! controlled through a command channel. Each load/seek spawns a decode thread that fills a
//! bounded channel (~4 s of audio); the rodio source drains it and outputs silence when it
//! runs dry (network hiccups), ending only on the decoder's explicit end marker.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    Load { path: PathBuf, start_ms: u64, end_ms: Option<u64>, autoplay: bool, epoch: u64 },
    Play,
    Pause,
    Seek(f64),
    Volume(f32),
    Stop,
}

pub struct Engine {
    tx: mpsc::Sender<Cmd>,
    /// bumped by every load/stop; events of an older track are dropped, so an `Ended`
    /// racing with a newer `load` cannot skip the new song
    epoch: Arc<AtomicU64>,
}

const TICK: Duration = Duration::from_millis(50);
const POS_EVERY: Duration = Duration::from_millis(250);
/// Each chunk in the prefetch channel holds ~100 ms; 40 of them ≈ 4 s.
const CHUNKS_PER_SEC: usize = 10;
const PREFETCH_CHUNKS: usize = 40;

impl Engine {
    pub fn spawn(on_event: Box<dyn Fn(EngineEvent) + Send>) -> Engine {
        let (tx, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let ep = epoch.clone();
        let spawned = std::thread::Builder::new().name("linger-audio".into()).spawn(move || run(rx, ep, on_event));
        if let Err(e) = spawned {
            eprintln!("linger: cannot start audio thread: {e}");
        }
        Engine { tx, epoch }
    }
    pub fn load(&self, path: PathBuf, start_ms: u64, end_ms: Option<u64>, autoplay: bool) {
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(Cmd::Load { path, start_ms, end_ms, autoplay, epoch });
    }
    pub fn play(&self) {
        self.send(Cmd::Play);
    }
    pub fn pause(&self) {
        self.send(Cmd::Pause);
    }
    /// `secs` from the start of the song (segment).
    pub fn seek(&self, secs: f64) {
        self.send(Cmd::Seek(secs));
    }
    /// 0..1 (applied as v², which sounds more linear)
    pub fn set_volume(&self, v: f32) {
        self.send(Cmd::Volume(v));
    }
    pub fn stop(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.send(Cmd::Stop);
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

struct StreamSource {
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
                return Some(s);
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
    fn restart(&mut self, secs: f64) {
        self.sink = None; // stops the old source; its receiver drops → decode thread exits
        self.shared = Arc::default();
        self.frame_samples = 0.0;
        self.offset = secs;
        let at = self.start_ms + (secs * 1000.0) as u64;
        self.pending = Some(spawn_decoder(self.path.clone(), at, self.end_ms));
    }
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

fn run(rx: Receiver<Cmd>, epoch: Arc<AtomicU64>, on_event: Box<dyn Fn(EngineEvent) + Send>) {
    let emit = |e| on_event(e);
    let current = |t: &Track| t.epoch == epoch.load(Ordering::SeqCst);
    let mut output = open_output();
    if output.is_none() {
        emit(EngineEvent::NoOutput);
    }
    let mut volume: f32 = 1.0;
    let mut track: Option<Track> = None;
    let mut last_pos = Instant::now();

    loop {
        match rx.recv_timeout(TICK) {
            Ok(Cmd::Load { path, start_ms, end_ms, autoplay, epoch: ep }) => {
                track = None; // drops the old sink + receivers → the old decode thread exits
                if output.is_none() {
                    output = open_output(); // a device may have been plugged in since
                }
                if output.is_none() {
                    emit(EngineEvent::NoOutput);
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
                };
                t.restart(0.0);
                track = Some(t);
            }
            Ok(Cmd::Play) => {
                if let Some(t) = track.as_mut() {
                    t.playing = true;
                    if let Some(s) = &t.sink {
                        s.play();
                    }
                }
            }
            Ok(Cmd::Pause) => {
                if let Some(t) = track.as_mut() {
                    t.playing = false;
                    if let Some(s) = &t.sink {
                        s.pause();
                    }
                }
            }
            Ok(Cmd::Seek(secs)) => {
                if let Some(t) = track.as_mut() {
                    t.restart(if secs.is_finite() { secs.max(0.0) } else { 0.0 });
                }
            }
            Ok(Cmd::Volume(v)) => {
                volume = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
                if let Some(s) = track.as_ref().and_then(|t| t.sink.as_ref()) {
                    s.set_volume(volume * volume);
                }
            }
            Ok(Cmd::Stop) => track = None,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let Some(t) = track.as_mut() else { continue };
        if !current(t) {
            continue; // superseded by a queued load/stop
        }

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
                    emit(EngineEvent::Error(e));
                    continue;
                }
            };
            let sink = match output.as_ref().map(|(_, h)| Sink::try_new(h)) {
                Some(Ok(s)) => s,
                _ => {
                    eprintln!("linger: cannot open an output sink");
                    output = None;
                    track = None;
                    emit(EngineEvent::NoOutput);
                    continue;
                }
            };
            sink.set_volume(volume * volume);
            if !t.playing {
                sink.pause();
            }
            t.frame_samples = rate as f64 * channels as f64;
            sink.append(StreamSource {
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
            emit(EngineEvent::Error(e));
            continue;
        }
        if t.shared.ended.load(Ordering::SeqCst) {
            if t.sink.as_ref().is_some_and(|s| !s.empty()) {
                continue; // the mixer is still draining what it pulled
            }
            let pos = t.position();
            track = None;
            emit(EngineEvent::Position(pos));
            emit(EngineEvent::Ended);
            continue;
        }
        if t.playing && t.sink.is_some() && last_pos.elapsed() >= POS_EVERY {
            last_pos = Instant::now();
            if t.shared.played.load(Ordering::Relaxed) > 0 {
                emit(EngineEvent::Position(t.position()));
            }
        }
    }
}
