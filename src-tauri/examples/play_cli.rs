//! Manual check of the playback engine (reads music only).
//!
//!   cargo run --example play_cli -- <file> [<file> ...]
//!
//! Plays the first 5 s of each file (start_ms 0, end_ms 5000) and reports the events.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use linger_lib::player::engine::{Engine, EngineEvent};

fn main() {
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if files.is_empty() {
        eprintln!("usage: play_cli <file> [<file> ...]");
        std::process::exit(2);
    }
    let (tx, rx) = mpsc::channel();
    let engine = Engine::spawn(Box::new(move |_epoch, ev| {
        let _ = tx.send(ev);
    }));
    engine.set_volume(0.5);
    let mut all_ok = true;
    for (n, f) in files.iter().enumerate() {
        let ext = f.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let t0 = Instant::now();
        engine.load(f.clone(), 0, Some(5000), true);
        let (mut positions, mut last_pos, mut first_pos_at) = (0, 0.0, None);
        let outcome = loop {
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(EngineEvent::Position(p)) => {
                    positions += 1;
                    last_pos = p;
                    first_pos_at.get_or_insert(t0.elapsed());
                }
                Ok(EngineEvent::Ended) => break "Ended".to_string(),
                Ok(EngineEvent::Error(e)) => break format!("Error({e})"),
                Ok(EngineEvent::NoOutput) => break "NoOutput".to_string(),
                Err(_) => break "timeout".to_string(),
            }
        };
        all_ok &= outcome == "Ended";
        println!(
            "#{n} .{ext}: {outcome} after {:.2}s; {positions} position events, first at {:?}, last pos {:.2}s",
            t0.elapsed().as_secs_f64(),
            first_pos_at.map(|d| format!("{:.2}s", d.as_secs_f64())),
            last_pos
        );
    }
    // segment + seek: play 60..70 s of the first file, seek to 5 s into the segment after 1 s
    let t0 = Instant::now();
    engine.load(files[0].clone(), 60_000, Some(70_000), true);
    let mut seeked = false;
    let (mut min_after_seek, mut last_pos) = (f64::MAX, 0.0);
    let outcome = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(EngineEvent::Position(p)) => {
                last_pos = p;
                if seeked {
                    min_after_seek = min_after_seek.min(p);
                }
            }
            Ok(EngineEvent::Ended) => break "Ended".to_string(),
            Ok(EngineEvent::Error(e)) => break format!("Error({e})"),
            Ok(EngineEvent::NoOutput) => break "NoOutput".to_string(),
            Err(_) if t0.elapsed() > Duration::from_secs(30) => break "timeout".to_string(),
            Err(_) => {}
        }
        if !seeked && t0.elapsed() > Duration::from_secs(1) {
            engine.seek(5.0);
            seeked = true;
        }
    };
    println!(
        "seek: {outcome} after {:.2}s (expect ~6 s); first pos after seek {:.2}, last pos {:.2} (expect ~5 and ~10)",
        t0.elapsed().as_secs_f64(),
        min_after_seek,
        last_pos
    );
    engine.stop();
    std::thread::sleep(Duration::from_millis(200));
    println!("{}", if all_ok { "ALL ENDED" } else { "SOME DID NOT END" });
}
