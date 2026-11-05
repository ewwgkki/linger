//! Audio decoding: symphonia first, FFmpeg (f32le pipe) for formats it cannot handle.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

pub use crate::ffmpeg::find_ffmpeg;

/// Extensions symphonia cannot decode at all; they go straight to FFmpeg.
const FFMPEG_ONLY: &[&str] = &["wma", "ape"];
/// Extensions symphonia normally handles; a failure there means a broken file.
const SYMPHONIA_EXTS: &[&str] = &["mp3", "mp2", "flac", "m4a", "mp4", "aac", "wav", "ogg"];
/// Output format of the FFmpeg pipe.
const FF_RATE: u32 = 44100;
const FF_CHANNELS: u16 = 2;
/// Bytes per FFmpeg read (100 ms of f32 stereo).
const FF_CHUNK_BYTES: usize = (FF_RATE as usize / 10) * FF_CHANNELS as usize * 4;
/// Consecutive undecodable packets before giving up on a file.
const MAX_BAD_PACKETS: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    NotFound,
    NeedFfmpeg,
    Unsupported,
    Io(String),
    Corrupt(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotFound => f.write_str("file not found"),
            DecodeError::NeedFfmpeg => f.write_str("format needs FFmpeg"),
            DecodeError::Unsupported => f.write_str("unsupported format"),
            DecodeError::Io(e) => write!(f, "I/O error: {e}"),
            DecodeError::Corrupt(e) => write!(f, "corrupt audio: {e}"),
        }
    }
}

/// Interleaved f32 samples, chunk by chunk.
pub trait Decoder: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// `Ok(None)` marks the end of the stream (or of the segment).
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, DecodeError>;
}

/// Opens `path` positioned at `start_ms`, ending at `end_ms` (or the end of the file).
/// Symphonia first; formats it cannot handle go through `ffmpeg` when given.
pub fn open_decoder(
    path: &Path,
    start_ms: u64,
    end_ms: Option<u64>,
    ffmpeg: Option<&Path>,
) -> Result<Box<dyn Decoder>, DecodeError> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(DecodeError::NotFound),
        Err(e) => return Err(io_err(e)),
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let sym_err = if FFMPEG_ONLY.contains(&ext.as_str()) {
        DecodeError::NeedFfmpeg
    } else {
        match SymphoniaDecoder::open(path, &ext, start_ms, end_ms) {
            Ok(d) => return Ok(Box::new(d)),
            Err(e @ (DecodeError::NotFound | DecodeError::Io(_))) => return Err(e),
            Err(e) => e,
        }
    };
    match ffmpeg {
        Some(ff) => FfmpegDecoder::open(ff, path, start_ms, end_ms).map(|d| Box::new(d) as Box<dyn Decoder>),
        None if sym_err == DecodeError::Unsupported && !SYMPHONIA_EXTS.contains(&ext.as_str()) => {
            Err(DecodeError::NeedFfmpeg)
        }
        None => Err(sym_err),
    }
}

fn io_err(e: std::io::Error) -> DecodeError {
    if e.kind() == std::io::ErrorKind::NotFound {
        DecodeError::NotFound
    } else {
        DecodeError::Io(e.to_string())
    }
}

fn sym_err(e: SymError) -> DecodeError {
    match e {
        SymError::IoError(e) => io_err(e),
        SymError::Unsupported(_) => DecodeError::Unsupported,
        e => DecodeError::Corrupt(e.to_string()),
    }
}

// ---------- symphonia ----------

struct SymphoniaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    channels: u16,
    start_ms: u64,
    end_ms: Option<u64>,
    /// first decoded chunk, read during `open` to learn the signal spec
    pending: Option<Vec<f32>>,
    buf: Option<(SignalSpec, usize, SampleBuffer<f32>)>,
    /// frame index (from the file start) following the last decoded packet, when ts is unknown
    next_frame: u64,
    done: bool,
}

impl SymphoniaDecoder {
    fn open(path: &Path, ext: &str, start_ms: u64, end_ms: Option<u64>) -> Result<SymphoniaDecoder, DecodeError> {
        let file = File::open(path).map_err(io_err)?;
        let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions { buffer_len: 1 << 20 });
        let mut hint = Hint::new();
        hint.with_extension(ext);
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(sym_err)?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or(DecodeError::Unsupported)?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(sym_err)?;
        let mut d = SymphoniaDecoder {
            time_base: params.time_base.filter(|tb| tb.numer > 0 && tb.denom > 0),
            rate: params.sample_rate.unwrap_or(0),
            channels: params.channels.map(|c| c.count() as u16).unwrap_or(0),
            format,
            decoder,
            track_id,
            start_ms,
            end_ms,
            pending: None,
            buf: None,
            next_frame: 0,
            done: false,
        };
        if start_ms > 0 {
            let to = SeekTo::Time { time: Time::from(start_ms as f64 / 1000.0), track_id: Some(track_id) };
            // unseekable streams fall back to decoding from the start and skipping
            if d.format.seek(SeekMode::Accurate, to).is_ok() {
                d.decoder.reset();
            }
        }
        d.pending = d.pull()?;
        if d.rate == 0 || d.channels == 0 {
            return Err(DecodeError::Corrupt("unknown sample format".into()));
        }
        Ok(d)
    }

    fn frame_of(time_base: Option<TimeBase>, ts: u64, rate: u32) -> u64 {
        match time_base {
            Some(tb) => (ts as u128 * tb.numer as u128 * rate as u128 / tb.denom as u128) as u64,
            None => ts,
        }
    }

    /// Next non-empty, trimmed chunk; `None` at the end.
    fn pull(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        let mut bad = 0;
        while !self.done {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    // a few broken bytes mid-file: keep what decoded so far
                    bad += 1;
                    if bad > MAX_BAD_PACKETS {
                        return Err(sym_err(e));
                    }
                    continue;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => {
                    bad += 1;
                    if bad > MAX_BAD_PACKETS {
                        return Err(DecodeError::Corrupt("too many undecodable packets".into()));
                    }
                    continue;
                }
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(sym_err(e)),
            };
            bad = 0;
            let spec = *decoded.spec();
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            if self.rate == 0 {
                self.rate = spec.rate;
            }
            if self.channels == 0 {
                self.channels = spec.channels.count() as u16;
            }
            let rate = self.rate;
            let f0 = if self.time_base.is_some() { Self::frame_of(self.time_base, packet.ts(), rate) } else { self.next_frame };
            self.next_frame = f0 + frames as u64;
            let start_f = self.start_ms * rate as u64 / 1000;
            let end_f = self.end_ms.map(|e| e * rate as u64 / 1000).unwrap_or(u64::MAX);
            if f0 >= end_f {
                self.done = true;
                break;
            }
            let lo = start_f.saturating_sub(f0).min(frames as u64) as usize;
            let hi = (end_f - f0).min(frames as u64) as usize;
            if hi <= lo {
                continue;
            }
            let reuse = matches!(&self.buf, Some((s, cap, _)) if *s == spec && *cap >= frames);
            if !reuse {
                self.buf = Some((spec, frames, SampleBuffer::new(frames as u64, spec)));
            }
            let (_, _, sb) = self.buf.as_mut().expect("buffer");
            sb.copy_interleaved_ref(decoded);
            let ch = spec.channels.count();
            let mut out = sb.samples()[lo * ch..hi * ch].to_vec();
            if ch != self.channels as usize {
                // mid-stream layout change: keep the announced channel count
                out = remix(&out, ch, self.channels as usize);
            }
            return Ok(Some(out));
        }
        self.done = true;
        Ok(None)
    }
}

fn remix(src: &[f32], from: usize, to: usize) -> Vec<f32> {
    src.chunks(from)
        .flat_map(|f| (0..to).map(move |c| f.get(c).or(f.first()).copied().unwrap_or(0.0)))
        .collect()
}

impl Decoder for SymphoniaDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        if let Some(c) = self.pending.take() {
            return Ok(Some(c));
        }
        self.pull()
    }
}

// ---------- FFmpeg ----------

struct FfmpegDecoder {
    child: Child,
    stdout: ChildStdout,
    stderr: Arc<Mutex<String>>,
    pending: Option<Vec<f32>>,
    produced: bool,
    done: bool,
}

impl FfmpegDecoder {
    fn open(ff: &Path, path: &Path, start_ms: u64, end_ms: Option<u64>) -> Result<FfmpegDecoder, DecodeError> {
        let mut cmd = Command::new(ff);
        cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{:.3}", start_ms as f64 / 1000.0)]);
        if let Some(end) = end_ms {
            cmd.args(["-t", &format!("{:.3}", end.saturating_sub(start_ms) as f64 / 1000.0)]);
        }
        cmd.arg("-i").arg(path);
        cmd.args(["-f", "f32le", "-ac", &FF_CHANNELS.to_string(), "-ar", &FF_RATE.to_string(), "pipe:1"]);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| DecodeError::Io(format!("ffmpeg: {e}")))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(mut err_pipe) = child.stderr.take() {
            let sink = stderr.clone();
            let _ = std::thread::Builder::new().name("linger-ffmpeg-stderr".into()).spawn(move || {
                let mut buf = Vec::new();
                let _ = err_pipe.read_to_end(&mut buf);
                let text = String::from_utf8_lossy(&buf);
                // keep it bounded
                let text: String = text.chars().take(2000).collect();
                *sink.lock().unwrap_or_else(|e| e.into_inner()) = text;
            });
        }
        let mut d = FfmpegDecoder { child, stdout, stderr, pending: None, produced: false, done: false };
        d.pending = d.read_chunk()?;
        Ok(d)
    }

    fn read_chunk(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        if self.done {
            return Ok(None);
        }
        let mut buf = vec![0u8; FF_CHUNK_BYTES];
        let mut n = 0;
        while n < buf.len() {
            match self.stdout.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(DecodeError::Io(e.to_string())),
            }
        }
        let n = n - n % (4 * FF_CHANNELS as usize);
        if n == 0 {
            self.done = true;
            let status = self.child.wait().map_err(|e| DecodeError::Io(e.to_string()))?;
            if !status.success() && !self.produced {
                // give the stderr reader a moment to finish
                std::thread::sleep(std::time::Duration::from_millis(50));
                let msg = self.stderr.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
                return Err(DecodeError::Corrupt(if msg.is_empty() { format!("ffmpeg exited with {status}") } else { msg }));
            }
            return Ok(None);
        }
        self.produced = true;
        Ok(Some(buf[..n].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()))
    }
}

impl Decoder for FfmpegDecoder {
    fn sample_rate(&self) -> u32 {
        FF_RATE
    }
    fn channels(&self) -> u16 {
        FF_CHANNELS
    }
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        if let Some(c) = self.pending.take() {
            return Ok(Some(c));
        }
        self.read_chunk()
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::testutil::{wav_pcm, write_temp};

    fn drain(d: &mut dyn Decoder) -> (usize, usize) {
        let (mut total, mut max_chunk) = (0, 0);
        while let Some(c) = d.next_chunk().expect("chunk") {
            total += c.len();
            max_chunk = max_chunk.max(c.len());
        }
        (total, max_chunk)
    }

    #[test]
    fn wav_decodes_full_length() {
        let p = write_temp("full.wav", &wav_pcm(2.0, 44100, &[]));
        let mut d = open_decoder(&p, 0, None, None).expect("open");
        assert_eq!((d.sample_rate(), d.channels()), (44100, 2));
        let (total, chunk) = drain(d.as_mut());
        let want = 2 * 44100 * 2;
        assert!(total.abs_diff(want) <= chunk, "total {total} want {want} chunk {chunk}");
        // a finished decoder stays finished
        assert_eq!(d.next_chunk(), Ok(None));
    }

    #[test]
    fn segment_decode_respects_start_end() {
        let p = write_temp("seg.wav", &wav_pcm(2.0, 44100, &[]));
        let mut d = open_decoder(&p, 500, Some(1500), None).expect("open");
        let (total, chunk) = drain(d.as_mut());
        let want = 44100 * 2;
        assert!(total.abs_diff(want) <= chunk, "total {total} want {want} chunk {chunk}");
        // open-ended segment from 1.5 s → 0.5 s left
        let mut d = open_decoder(&p, 1500, None, None).expect("open");
        let (total, chunk) = drain(d.as_mut());
        assert!(total.abs_diff(44100) <= chunk, "total {total} chunk {chunk}");
        // start beyond the end → nothing, but no error
        let mut d = open_decoder(&p, 5000, None, None).expect("open");
        assert_eq!(drain(d.as_mut()).0, 0);
    }

    #[test]
    fn missing_file_returns_error() {
        let p = std::env::temp_dir().join("linger-definitely-missing").join("gone.flac");
        assert_eq!(open_decoder(&p, 0, None, None).err(), Some(DecodeError::NotFound));
        let p = std::env::temp_dir().join("linger-definitely-missing").join("gone.wma");
        assert_eq!(open_decoder(&p, 0, None, Some(Path::new("ffmpeg"))).err(), Some(DecodeError::NotFound));
    }

    #[test]
    fn corrupt_file_errors_without_panic() {
        let mut x = 0x1234_5678u32;
        let bytes: Vec<u8> = (0..64 * 1024)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let p = write_temp("x.flac", &bytes);
        assert!(open_decoder(&p, 0, None, None).is_err());
        // a real header followed by garbage: open may succeed, decoding must not panic
        let mut wav = wav_pcm(1.0, 44100, &[]);
        wav.truncate(100);
        let p = write_temp("t.wav", &wav);
        if let Ok(mut d) = open_decoder(&p, 0, None, None) {
            while let Ok(Some(_)) = d.next_chunk() {}
        }
    }

    #[test]
    fn wma_without_ffmpeg_needs_ffmpeg() {
        let p = write_temp("a.wma", &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0, 0, 0, 0]);
        assert_eq!(open_decoder(&p, 0, None, None).err(), Some(DecodeError::NeedFfmpeg));
    }

    #[test]
    #[ignore]
    fn ffmpeg_decodes_wma() {
        let path = std::env::var("LINGER_TEST_WMA").expect("LINGER_TEST_WMA");
        let ff = find_ffmpeg().expect("ffmpeg");
        let mut d = open_decoder(Path::new(&path), 1000, Some(4000), Some(&ff)).expect("open");
        assert_eq!((d.sample_rate(), d.channels()), (44100, 2));
        let (total, chunk) = drain(d.as_mut());
        let want = 3 * 44100 * 2;
        assert!(total.abs_diff(want) <= chunk.max(4410), "total {total} want {want}");
        assert!(total > 0);
    }

    #[test]
    fn ffmpeg_failure_is_an_error() {
        // a fake "ffmpeg" that cannot run → an error, not a panic or silent success
        let p = write_temp("b.ape", b"MAC \x96\x0f\0\0garbage");
        let fake = write_temp(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }, b"not a program");
        assert!(open_decoder(&p, 0, None, Some(&fake)).is_err());
    }
}
