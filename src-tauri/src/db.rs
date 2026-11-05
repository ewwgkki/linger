//! SQLite library store.
//!
//! Song ids are plain TEXT; future playlist/favorite tables must reference them
//! WITHOUT a foreign key so that replacing a file's songs never cascades.

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema is newer than this app (v{0})")]
    TooNew(i64),
}
pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, Clone, PartialEq)]
pub struct Root { pub id: i64, pub path: String }
#[derive(Debug, Clone, PartialEq)]
pub struct FileSig { pub id: i64, pub size: u64, pub mtime: i64, pub cue_sig: Option<String> }
#[derive(Debug, Clone, PartialEq)]
pub struct FileMeta {
    pub rel: String, pub ext: String, pub size: u64, pub mtime: i64, pub codec: String,
    pub bitrate: u32, pub duration: f64, pub cover: u8, pub cue_sig: Option<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NewSong {
    pub id: String, pub title: String, pub artist: String, pub album: String, pub track: u32,
    pub start_ms: u64, pub end_ms: Option<u64>, pub duration: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SongRow {
    pub id: String, pub root_id: i64, pub file_id: i64, pub rel: String, pub ext: String, pub codec: String,
    pub bitrate: u32, pub size: u64, pub cover: u8, pub title: String, pub artist: String, pub album: String,
    pub track: u32, pub start_ms: u64, pub end_ms: Option<u64>, pub duration: f64,
}

/// Each entry is one schema version (index + 1), applied atomically.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE roots(id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE);
     CREATE TABLE files(
        id INTEGER PRIMARY KEY,
        root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
        rel TEXT NOT NULL, ext TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL,
        codec TEXT NOT NULL, bitrate INTEGER NOT NULL, duration REAL NOT NULL,
        cover INTEGER NOT NULL DEFAULT 0, cue_sig TEXT,
        UNIQUE(root_id, rel));
     CREATE TABLE songs(
        id TEXT PRIMARY KEY,
        file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT NOT NULL, track INTEGER NOT NULL,
        start_ms INTEGER NOT NULL, end_ms INTEGER, duration REAL NOT NULL);
     CREATE INDEX songs_file ON songs(file_id);
     CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
];

fn schema_version(c: &Connection) -> Result<i64> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
    let v: Option<String> = c
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

fn migrate_with(c: &mut Connection, migrations: &[&str]) -> Result<()> {
    let cur = schema_version(c)?;
    if cur > migrations.len() as i64 {
        return Err(DbError::TooNew(cur));
    }
    for (i, sql) in migrations.iter().enumerate().skip(cur as usize) {
        let tx = c.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('schema_version',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [(i + 1).to_string()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

pub struct Db { conn: Connection }

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }
    pub fn open_in_memory() -> Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(mut conn: Connection) -> Result<Db> {
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate_with(&mut conn, MIGRATIONS)?;
        Ok(Db { conn })
    }

    pub fn roots(&self) -> Result<Vec<Root>> {
        let mut st = self.conn.prepare("SELECT id, path FROM roots ORDER BY id")?;
        let v = st.query_map([], |r| Ok(Root { id: r.get(0)?, path: r.get(1)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    pub fn add_root(&self, path: &str) -> Result<Root> {
        self.conn.execute("INSERT OR IGNORE INTO roots(path) VALUES(?1)", [path])?;
        let id = self.conn.query_row("SELECT id FROM roots WHERE path=?1", [path], |r| r.get(0))?;
        Ok(Root { id, path: path.to_string() })
    }
    pub fn remove_root(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM roots WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn file_sigs(&self, root_id: i64) -> Result<HashMap<String, FileSig>> {
        let mut st = self.conn.prepare("SELECT rel, id, size, mtime, cue_sig FROM files WHERE root_id=?1")?;
        let rows = st.query_map([root_id], |r| {
            Ok((r.get::<_, String>(0)?, FileSig {
                id: r.get(1)?, size: r.get::<_, i64>(2)? as u64, mtime: r.get(3)?, cue_sig: r.get(4)?,
            }))
        })?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }
    pub fn replace_file(&mut self, root_id: i64, m: &FileMeta, songs: &[NewSong]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO files(root_id, rel, ext, size, mtime, codec, bitrate, duration, cover, cue_sig)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(root_id, rel) DO UPDATE SET ext=excluded.ext, size=excluded.size,
               mtime=excluded.mtime, codec=excluded.codec, bitrate=excluded.bitrate,
               duration=excluded.duration, cover=excluded.cover, cue_sig=excluded.cue_sig",
            params![root_id, m.rel, m.ext, m.size as i64, m.mtime, m.codec, m.bitrate, m.duration, m.cover, m.cue_sig],
        )?;
        let file_id: i64 = tx.query_row(
            "SELECT id FROM files WHERE root_id=?1 AND rel=?2", params![root_id, m.rel], |r| r.get(0))?;
        tx.execute("DELETE FROM songs WHERE file_id=?1", [file_id])?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO songs(id, file_id, title, artist, album, track, start_ms, end_ms, duration)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
            for s in songs {
                ins.execute(params![s.id, file_id, s.title, s.artist, s.album, s.track,
                    s.start_ms as i64, s.end_ms.map(|v| v as i64), s.duration])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn delete_files(&mut self, root_id: i64, rels: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM files WHERE root_id=?1 AND rel=?2")?;
            for rel in rels {
                del.execute(params![root_id, rel])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    const SONG_SELECT: &'static str =
        "SELECT s.id, f.root_id, s.file_id, f.rel, f.ext, f.codec, f.bitrate, f.size, f.cover,
                s.title, s.artist, s.album, s.track, s.start_ms, s.end_ms, s.duration
         FROM songs s JOIN files f ON f.id = s.file_id";

    fn map_song(r: &rusqlite::Row) -> rusqlite::Result<SongRow> {
        Ok(SongRow {
            id: r.get(0)?, root_id: r.get(1)?, file_id: r.get(2)?, rel: r.get(3)?, ext: r.get(4)?,
            codec: r.get(5)?, bitrate: r.get(6)?, size: r.get::<_, i64>(7)? as u64, cover: r.get(8)?,
            title: r.get(9)?, artist: r.get(10)?, album: r.get(11)?, track: r.get(12)?,
            start_ms: r.get::<_, i64>(13)? as u64, end_ms: r.get::<_, Option<i64>>(14)?.map(|v| v as u64),
            duration: r.get(15)?,
        })
    }
    pub fn all_songs(&self) -> Result<Vec<SongRow>> {
        let mut st = self.conn.prepare(&format!("{} ORDER BY s.rowid", Self::SONG_SELECT))?;
        let v = st.query_map([], Self::map_song)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    pub fn song(&self, id: &str) -> Result<Option<SongRow>> {
        let mut st = self.conn.prepare(&format!("{} WHERE s.id=?1", Self::SONG_SELECT))?;
        Ok(st.query_row([id], Self::map_song).optional()?)
    }
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [key, value])?;
        Ok(())
    }
}

/// sha1(root_path \0 rel \0 start_ms), first 16 hex chars. `rel` separators normalized to '/'.
pub fn song_id(root_path: &str, rel: &str, start_ms: u64) -> String {
    let rel = rel.replace('\\', "/");
    let mut h = sha1_smol::Sha1::new();
    h.update(root_path.as_bytes());
    h.update(b"\0");
    h.update(rel.as_bytes());
    h.update(b"\0");
    h.update(start_ms.to_string().as_bytes());
    h.digest().to_string()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(rel: &str) -> FileMeta {
        FileMeta { rel: rel.into(), ext: "mp3".into(), size: 10, mtime: 5, codec: "mp3".into(),
                   bitrate: 320, duration: 100.0, cover: 1, cue_sig: None }
    }
    fn song(root: &str, rel: &str, start: u64, n: u32) -> NewSong {
        NewSong { id: song_id(root, rel, start), title: format!("t{n}"), artist: "a".into(), album: "b".into(),
                  track: n, start_ms: start, end_ms: None, duration: 1.5 }
    }

    #[test] fn add_root_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let a = db.add_root("D:/m").unwrap(); let b = db.add_root("D:/m").unwrap();
        assert_eq!(a.id, b.id); assert_eq!(db.roots().unwrap().len(), 1);
    }
    #[test] fn replace_file_replaces_songs() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.add_root("D:/m").unwrap();
        db.replace_file(r.id, &meta("a.flac"), &[song("D:/m", "a.flac", 0, 1)]).unwrap();
        assert_eq!(db.all_songs().unwrap().len(), 1);
        let three: Vec<_> = (0..3).map(|i| song("D:/m", "a.flac", i * 1000, i as u32 + 1)).collect();
        db.replace_file(r.id, &meta("a.flac"), &three).unwrap();
        assert_eq!(db.all_songs().unwrap().len(), 3);
        let got = db.song(&three[1].id).unwrap().unwrap();
        assert_eq!(got.start_ms, 1000); assert_eq!(got.rel, "a.flac"); assert_eq!(got.cover, 1);
        assert!(db.song("nope").unwrap().is_none());
        assert_eq!(db.file_sigs(r.id).unwrap().len(), 1);
    }
    #[test] fn delete_files_removes_songs() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.add_root("D:/m").unwrap();
        db.replace_file(r.id, &meta("a.mp3"), &[song("D:/m", "a.mp3", 0, 1)]).unwrap();
        db.replace_file(r.id, &meta("b.mp3"), &[song("D:/m", "b.mp3", 0, 1)]).unwrap();
        db.delete_files(r.id, &["a.mp3".to_string()]).unwrap();
        assert_eq!(db.all_songs().unwrap().len(), 1);
    }
    #[test] fn remove_root_cascades() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.add_root("D:/m").unwrap();
        db.replace_file(r.id, &meta("a.mp3"), &[song("D:/m", "a.mp3", 0, 1)]).unwrap();
        db.remove_root(r.id).unwrap();
        assert!(db.all_songs().unwrap().is_empty());
        assert!(db.roots().unwrap().is_empty());
    }
    #[test] fn settings_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.setting("lang").unwrap(), None);
        db.set_setting("lang", "en").unwrap();
        db.set_setting("lang", "zh").unwrap();
        assert_eq!(db.setting("lang").unwrap(), Some("zh".into()));
    }
    #[test] fn song_id_is_stable() {
        assert_eq!(song_id("D:/m", "a/b.mp3", 0), song_id("D:/m", "a/b.mp3", 0));
        assert_ne!(song_id("D:/m", "a/b.mp3", 0), song_id("D:/m", "a/b.mp3", 1000));
        assert_eq!(song_id("D:/m", "a\\b.mp3", 0), song_id("D:/m", "a/b.mp3", 0));
        assert_eq!(song_id("D:/m", "a/b.mp3", 0).len(), 16);
    }
    #[test] fn ten_thousand_songs_load_fast() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.add_root("D:/m").unwrap();
        for i in 0..10000u32 {
            let rel = format!("f{i}.mp3");
            db.replace_file(r.id, &meta(&rel), &[song("D:/m", &rel, 0, i)]).unwrap();
        }
        let t = std::time::Instant::now();
        assert_eq!(db.all_songs().unwrap().len(), 10000);
        assert!(t.elapsed().as_secs_f64() < 1.0, "{:?}", t.elapsed());
    }
    #[test] fn reopen_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("lib.db");
        {
            let mut db = Db::open(&p).unwrap();
            let r = db.add_root("D:/m").unwrap();
            db.replace_file(r.id, &meta("a.mp3"), &[song("D:/m", "a.mp3", 0, 1)]).unwrap();
            db.set_setting("k", "v").unwrap();
        }
        let db = Db::open(&p).unwrap();
        assert_eq!(db.all_songs().unwrap().len(), 1);
        assert_eq!(db.setting("k").unwrap(), Some("v".into()));
        assert_eq!(db.roots().unwrap().len(), 1);
    }
    #[test] fn migration_failure_rolls_back() {
        let mut c = rusqlite::Connection::open_in_memory().unwrap();
        let bad: &[&str] = &["CREATE TABLE y(a);", "CREATE TABLE x(a); CREATE TABLE y(a);"];
        assert!(migrate_with(&mut c, bad).is_err());
        let n: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name='x'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0); // v2 rolled back entirely
        assert_eq!(schema_version(&c).unwrap(), 1); // v1 kept
    }
}
