//! Manual smoke test of the scanner against a real library (reads music only).
//!
//!   cargo run --example scan_cli -- [--fresh] <root> [<root> ...]
//!
//! The database lives in `<temp>/linger-scan-cli/linger.db` (kept between runs so a
//! second run shows the incremental path); the report is also written as UTF-8 to
//! `<temp>/linger-scan-cli/report.txt` because Windows consoles garble CJK text.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::time::Instant;

use linger_lib::commands::library::{normalize_root, probe_roots};
use linger_lib::db::Db;
use linger_lib::scan::scan;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let fresh = args.iter().any(|a| a == "--fresh");
    args.retain(|a| a != "--fresh");
    let dir = std::env::temp_dir().join("linger-scan-cli");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let db_path = dir.join("linger.db");
    if fresh {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("linger.db{ext}")));
        }
    }
    let mut db = Db::open(&db_path).expect("open db");
    for a in &args {
        db.add_root(&normalize_root(a)).expect("add root");
    }
    let roots = db.roots().expect("roots");
    let mut out = String::new();

    let t0 = Instant::now();
    let online: HashSet<i64> = probe_roots(&roots);
    let probe = t0.elapsed();
    let t1 = Instant::now();
    let mut ticks = 0usize;
    let rep = scan(&mut db, &roots, &online, &mut |_| ticks += 1);
    let took = t1.elapsed();

    writeln!(out, "db: {}", db_path.display()).unwrap();
    for r in &roots {
        writeln!(out, "root {} online={}", r.path, online.contains(&r.id)).unwrap();
    }
    writeln!(out, "probe: {:.2?}  scan: {:.2?}  progress callbacks: {ticks}", probe, took).unwrap();
    writeln!(out, "songs: {}  read: {}  skipped: {}", rep.count, rep.read, rep.skipped.len()).unwrap();
    for s in &rep.skipped {
        writeln!(out, "  skipped [{}] {}", s.why, s.path).unwrap();
    }

    let t2 = Instant::now();
    let songs = db.all_songs().expect("songs");
    let load = t2.elapsed();
    writeln!(out, "all_songs: {} rows in {:.2?}", songs.len(), load).unwrap();
    let mut split: BTreeMap<(i64, String), Vec<_>> = BTreeMap::new();
    for s in songs.iter().filter(|s| s.end_ms.is_some() || s.start_ms > 0) {
        split.entry((s.root_id, s.rel.clone())).or_default().push(s);
    }
    writeln!(out, "CUE-split files: {}", split.len()).unwrap();
    for ((_, rel), v) in &split {
        let first: Vec<String> = v.iter().take(2).map(|s| format!("{} ({:.1}s)", s.title, s.duration)).collect();
        let last = v.last().unwrap();
        writeln!(out, "  {} -> {} tracks [{} / {}] artist={} album={} last={:.1}s",
            rel, v.len(), first.join(", "), v.len(), last.artist, last.album, last.duration).unwrap();
    }
    print!("{out}");
    std::fs::write(dir.join("report.txt"), &out).expect("write report");
}
