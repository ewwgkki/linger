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
    #[error("not found")]
    NotFound,
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
    /// the audio file's mtime (unix seconds) as of the last scan
    pub mtime: i64,
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
    // v2: playlists and favorites. Deliberately NO foreign key to songs: items keep a
    // snapshot and must survive file deletion, rescans and root removal.
    "CREATE TABLE playlists(id INTEGER PRIMARY KEY, name TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL);
     CREATE TABLE playlist_items(
       id INTEGER PRIMARY KEY,
       playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
       pos INTEGER NOT NULL, song_id TEXT NOT NULL,
       title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT NOT NULL, duration REAL NOT NULL, size INTEGER NOT NULL,
       added INTEGER NOT NULL,
       UNIQUE(playlist_id, song_id));
     CREATE INDEX playlist_items_song ON playlist_items(song_id);
     CREATE TABLE favorites(song_id TEXT PRIMARY KEY,
       title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT NOT NULL, duration REAL NOT NULL, size INTEGER NOT NULL,
       added INTEGER NOT NULL);",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot { pub title: String, pub artist: String, pub album: String, pub duration: f64, pub size: u64 }
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistRow { pub id: i64, pub name: String, pub count: usize, pub duration: f64, pub created: i64, pub updated: i64 }
#[derive(Debug, Clone, PartialEq)]
pub struct PlItem { pub item_id: i64, pub song_id: String, pub snap: Snapshot, pub added: i64 }
#[derive(Debug, Clone, PartialEq)]
pub struct FavRow { pub song_id: String, pub snap: Snapshot, pub added: i64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddOutcome { pub added: usize, pub duplicate: usize, pub unknown: usize }

fn snap_of(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Snapshot> {
    Ok(Snapshot { title: r.get(i)?, artist: r.get(i + 1)?, album: r.get(i + 2)?, duration: r.get(i + 3)?,
                  size: r.get::<_, i64>(i + 4)? as u64 })
}
/// Snapshot of a song currently in the library (size = its file's size).
fn lib_snapshot(c: &Connection, id: &str) -> rusqlite::Result<Option<Snapshot>> {
    c.query_row(
        "SELECT s.title, s.artist, s.album, s.duration, f.size FROM songs s JOIN files f ON f.id=s.file_id WHERE s.id=?1",
        [id], |r| snap_of(r, 0)).optional()
}
fn playlist_has(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(c.query_row("SELECT count(*) FROM playlists WHERE id=?1", [id], |r| r.get::<_, i64>(0))? > 0)
}
fn touch(c: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE playlists SET updated=?2 WHERE id=?1", params![id, now])?;
    Ok(())
}
/// remove/move take no `now` argument, so they stamp `updated` with the SQLite clock.
fn touch_now(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE playlists SET updated=CAST(strftime('%s','now') AS INTEGER) WHERE id=?1", [id])?;
    Ok(())
}
fn pl_add(c: &Connection, id: i64, song_ids: &[String], now: i64) -> Result<AddOutcome> {
    if !playlist_has(c, id)? { return Err(DbError::NotFound); }
    let mut pos: i64 = c.query_row("SELECT COALESCE(MAX(pos)+1, 0) FROM playlist_items WHERE playlist_id=?1", [id], |r| r.get(0))?;
    let mut out = AddOutcome { added: 0, duplicate: 0, unknown: 0 };
    for sid in song_ids {
        let dup: i64 = c.query_row("SELECT count(*) FROM playlist_items WHERE playlist_id=?1 AND song_id=?2",
                                   params![id, sid], |r| r.get(0))?;
        if dup > 0 { out.duplicate += 1; continue; }
        let Some(sn) = lib_snapshot(c, sid)? else { out.unknown += 1; continue; };
        c.execute(
            "INSERT INTO playlist_items(playlist_id,pos,song_id,title,artist,album,duration,size,added)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id, pos, sid, sn.title, sn.artist, sn.album, sn.duration, sn.size as i64, now])?;
        pos += 1; out.added += 1;
    }
    if out.added > 0 { touch(c, id, now)?; }
    Ok(out)
}
fn pl_remove_songs(c: &Connection, id: i64, song_ids: &[String], now: i64) -> Result<usize> {
    if !playlist_has(c, id)? { return Err(DbError::NotFound); }
    let mut n = 0;
    for sid in song_ids {
        n += c.execute("DELETE FROM playlist_items WHERE playlist_id=?1 AND song_id=?2", params![id, sid])?;
    }
    if n > 0 { touch(c, id, now)?; }
    Ok(n)
}
fn fav_add(c: &Connection, song_ids: &[String], now: i64) -> Result<AddOutcome> {
    let mut out = AddOutcome { added: 0, duplicate: 0, unknown: 0 };
    for sid in song_ids {
        let dup: i64 = c.query_row("SELECT count(*) FROM favorites WHERE song_id=?1", [sid], |r| r.get(0))?;
        if dup > 0 { out.duplicate += 1; continue; }
        let Some(sn) = lib_snapshot(c, sid)? else { out.unknown += 1; continue; };
        c.execute(
            "INSERT INTO favorites(song_id,title,artist,album,duration,size,added) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![sid, sn.title, sn.artist, sn.album, sn.duration, sn.size as i64, now])?;
        out.added += 1;
    }
    Ok(out)
}
fn fav_remove(c: &Connection, song_ids: &[String]) -> Result<usize> {
    let mut n = 0;
    for sid in song_ids { n += c.execute("DELETE FROM favorites WHERE song_id=?1", [sid])?; }
    Ok(n)
}
fn norm(s: &str) -> String { s.trim().to_lowercase() }
/// Library songs indexed by file size, built once per relink pass.
type SizeIndex = HashMap<u64, Vec<(String, Snapshot)>>;
fn size_index(c: &Connection) -> Result<SizeIndex> {
    let mut st = c.prepare(
        "SELECT s.id, s.title, s.artist, s.album, s.duration, f.size FROM songs s JOIN files f ON f.id=s.file_id")?;
    let mut idx: SizeIndex = HashMap::new();
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, snap_of(r, 1)?)))? {
        let (id, sn) = row?;
        idx.entry(sn.size).or_default().push((id, sn));
    }
    Ok(idx)
}
/// The single library song matching a snapshot (same file size, duration within 1s,
/// same trimmed case-insensitive title and artist); None if zero or several match.
fn unique_candidate(idx: &SizeIndex, sn: &Snapshot) -> Option<(String, Snapshot)> {
    let (t, a) = (norm(&sn.title), norm(&sn.artist));
    let mut m = idx.get(&sn.size)?.iter()
        .filter(|(_, x)| (x.duration - sn.duration).abs() <= 1.0 && norm(&x.title) == t && norm(&x.artist) == a);
    let first = m.next();
    if m.next().is_some() { None } else { first.cloned() }
}

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
        // FULL so playlist edits survive power loss; scans switch to NORMAL via set_bulk
        conn.pragma_update(None, "synchronous", "FULL")?;
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

    #[cfg(test)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
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
                s.title, s.artist, s.album, s.track, s.start_ms, s.end_ms, s.duration, f.mtime
         FROM songs s JOIN files f ON f.id = s.file_id";

    fn map_song(r: &rusqlite::Row) -> rusqlite::Result<SongRow> {
        Ok(SongRow {
            id: r.get(0)?, root_id: r.get(1)?, file_id: r.get(2)?, rel: r.get(3)?, ext: r.get(4)?,
            codec: r.get(5)?, bitrate: r.get(6)?, size: r.get::<_, i64>(7)? as u64, cover: r.get(8)?,
            title: r.get(9)?, artist: r.get(10)?, album: r.get(11)?, track: r.get(12)?,
            start_ms: r.get::<_, i64>(13)? as u64, end_ms: r.get::<_, Option<i64>>(14)?.map(|v| v as u64),
            duration: r.get(15)?, mtime: r.get(16)?,
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
    /// (root path, rel, size, mtime) of every file that has a cover (embedded or folder image).
    pub fn cover_files(&self) -> Result<Vec<(String, String, u64, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT r.path, f.rel, f.size, f.mtime FROM files f JOIN roots r ON r.id = f.root_id WHERE f.cover IN (1,2)")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    pub fn song_count(&self) -> Result<usize> {
        Ok(self.conn.query_row("SELECT count(*) FROM songs", [], |r| r.get::<_, i64>(0))? as usize)
    }
    /// Songs per root id (roots without songs are absent).
    pub fn root_song_counts(&self) -> Result<HashMap<i64, usize>> {
        let mut st = self.conn.prepare(
            "SELECT f.root_id, count(*) FROM songs s JOIN files f ON f.id = s.file_id GROUP BY f.root_id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }
    /// Bulk mode (scanning): `on` relaxes fsync to NORMAL, off restores FULL.
    pub fn set_bulk(&self, on: bool) -> Result<()> {
        self.conn.pragma_update(None, "synchronous", if on { "NORMAL" } else { "FULL" })?;
        Ok(())
    }

    pub fn playlists(&self) -> Result<Vec<PlaylistRow>> {
        let mut st = self.conn.prepare(
            "SELECT p.id, p.name, count(i.id), COALESCE(sum(i.duration),0), p.created, p.updated
             FROM playlists p LEFT JOIN playlist_items i ON i.playlist_id=p.id
             GROUP BY p.id ORDER BY p.created, p.id")?;
        let v = st.query_map([], |r| Ok(PlaylistRow {
            id: r.get(0)?, name: r.get(1)?, count: r.get::<_, i64>(2)? as usize, duration: r.get(3)?,
            created: r.get(4)?, updated: r.get(5)? }))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    pub fn create_playlist(&self, name: &str, now: i64) -> Result<i64> {
        self.conn.execute("INSERT INTO playlists(name,created,updated) VALUES(?1,?2,?2)", params![name, now])?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn rename_playlist(&self, id: i64, name: &str, now: i64) -> Result<bool> {
        Ok(self.conn.execute("UPDATE playlists SET name=?2, updated=?3 WHERE id=?1", params![id, name, now])? > 0)
    }
    pub fn delete_playlist(&self, id: i64) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM playlists WHERE id=?1", [id])? > 0)
    }
    pub fn playlist_exists(&self, id: i64) -> Result<bool> {
        Ok(playlist_has(&self.conn, id)?)
    }
    pub fn playlist_items(&self, id: i64) -> Result<Vec<PlItem>> {
        let mut st = self.conn.prepare(
            "SELECT id, song_id, title, artist, album, duration, size, added FROM playlist_items
             WHERE playlist_id=?1 ORDER BY pos, id")?;
        let v = st.query_map([id], |r| Ok(PlItem { item_id: r.get(0)?, song_id: r.get(1)?, snap: snap_of(r, 2)?, added: r.get(7)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    /// Appends songs (in order) after the current last position; duplicates and ids not in the library are skipped.
    pub fn playlist_add(&mut self, id: i64, song_ids: &[String], now: i64) -> Result<AddOutcome> {
        let tx = self.conn.transaction()?;
        let out = pl_add(&tx, id, song_ids, now)?;
        tx.commit()?;
        Ok(out)
    }
    pub fn playlist_remove(&mut self, id: i64, item_ids: &[i64]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for it in item_ids {
            n += tx.execute("DELETE FROM playlist_items WHERE playlist_id=?1 AND id=?2", params![id, it])?;
        }
        if n > 0 { touch_now(&tx, id)?; }
        tx.commit()?;
        Ok(n)
    }
    pub fn playlist_move(&mut self, id: i64, item_id: i64, to: usize) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut order: Vec<i64> = {
            let mut st = tx.prepare("SELECT id FROM playlist_items WHERE playlist_id=?1 ORDER BY pos, id")?;
            let v = st.query_map([id], |r| r.get(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        let Some(from) = order.iter().position(|x| *x == item_id) else { return Ok(false) };
        order.remove(from);
        order.insert(to.min(order.len()), item_id);
        for (pos, iid) in order.iter().enumerate() {
            tx.execute("UPDATE playlist_items SET pos=?2 WHERE id=?1", params![iid, pos as i64])?;
        }
        touch_now(&tx, id)?;
        tx.commit()?;
        Ok(true)
    }
    pub fn favorites(&self) -> Result<Vec<FavRow>> {
        let mut st = self.conn.prepare(
            "SELECT song_id, title, artist, album, duration, size, added FROM favorites ORDER BY added DESC, rowid DESC")?;
        let v = st.query_map([], |r| Ok(FavRow { song_id: r.get(0)?, snap: snap_of(r, 1)?, added: r.get(6)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    /// on: add (returns newly added); off: remove (returns removed).
    pub fn set_favorites(&mut self, song_ids: &[String], on: bool, now: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = if on { fav_add(&tx, song_ids, now)?.added } else { fav_remove(&tx, song_ids)? };
        tx.commit()?;
        Ok(n)
    }
    /// Songs in favorites or any playlist (deduplicated).
    pub fn saved_song_ids(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT song_id FROM favorites UNION SELECT song_id FROM playlist_items")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }
    /// (how many of `song_ids` are favorites, [(playlist id, how many are in it)]) for every playlist.
    pub fn memberships(&self, song_ids: &[String]) -> Result<(usize, Vec<(i64, usize)>)> {
        let mut fav = 0;
        let mut per: HashMap<i64, usize> = HashMap::new();
        let mut f = self.conn.prepare("SELECT count(*) FROM favorites WHERE song_id=?1")?;
        let mut p = self.conn.prepare("SELECT playlist_id FROM playlist_items WHERE song_id=?1")?;
        let uniq: std::collections::HashSet<&String> = song_ids.iter().collect();
        for sid in uniq {
            fav += f.query_row([sid], |r| r.get::<_, i64>(0))? as usize;
            for pid in p.query_map([sid], |r| r.get::<_, i64>(0))? { *per.entry(pid?).or_insert(0) += 1; }
        }
        let list = self.playlists()?.into_iter().map(|pl| (pl.id, per.get(&pl.id).copied().unwrap_or(0))).collect();
        Ok((fav, list))
    }
    /// Adds to `add` targets and removes from `remove` targets in one transaction (target 0 = favorites).
    /// An unknown playlist id fails the whole call. Returns (added, already present, removed).
    pub fn save_songs(&mut self, song_ids: &[String], add: &[i64], remove: &[i64], now: i64) -> Result<(usize, usize, usize)> {
        let tx = self.conn.transaction()?;
        let (mut added, mut dup, mut removed) = (0, 0, 0);
        for t in add {
            let o = if *t == 0 { fav_add(&tx, song_ids, now)? } else { pl_add(&tx, *t, song_ids, now)? };
            added += o.added; dup += o.duplicate;
        }
        for t in remove {
            removed += if *t == 0 { fav_remove(&tx, song_ids)? } else { pl_remove_songs(&tx, *t, song_ids, now)? };
        }
        tx.commit()?;
        Ok((added, dup, removed))
    }
    /// Re-points favorites / playlist items whose song id is no longer in the library to the
    /// one library song matching their snapshot (renamed or moved file). Ambiguous matches are left alone.
    /// Returns the number of rows (items + favorites) relinked.
    pub fn relink_missing(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        let favs: Vec<(String, Snapshot)> = {
            let mut st = tx.prepare(
                "SELECT song_id, title, artist, album, duration, size FROM favorites
                 WHERE song_id NOT IN (SELECT id FROM songs)")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, snap_of(r, 1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        let items: Vec<(i64, i64, Snapshot)> = {
            let mut st = tx.prepare(
                "SELECT id, playlist_id, title, artist, album, duration, size, song_id FROM playlist_items
                 WHERE song_id NOT IN (SELECT id FROM songs) ORDER BY id")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, snap_of(r, 2)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        if favs.is_empty() && items.is_empty() { return Ok(0); }
        let idx = size_index(&tx)?;
        for (old, sn) in favs {
            let Some((new, ns)) = unique_candidate(&idx, &sn) else { continue };
            tx.execute(
                "INSERT OR IGNORE INTO favorites(song_id,title,artist,album,duration,size,added)
                 SELECT ?2,?3,?4,?5,?6,?7,added FROM favorites WHERE song_id=?1",
                params![old, new, ns.title, ns.artist, ns.album, ns.duration, ns.size as i64])?;
            tx.execute("DELETE FROM favorites WHERE song_id=?1", [&old])?;
            n += 1;
        }
        for (item, pl, sn) in items {
            let Some((new, ns)) = unique_candidate(&idx, &sn) else { continue };
            let exists: i64 = tx.query_row(
                "SELECT count(*) FROM playlist_items WHERE playlist_id=?1 AND song_id=?2", params![pl, new], |r| r.get(0))?;
            if exists > 0 {
                tx.execute("DELETE FROM playlist_items WHERE id=?1", [item])?;
            } else {
                tx.execute(
                    "UPDATE playlist_items SET song_id=?2, title=?3, artist=?4, album=?5, duration=?6, size=?7 WHERE id=?1",
                    params![item, new, ns.title, ns.artist, ns.album, ns.duration, ns.size as i64])?;
            }
            n += 1;
        }
        tx.commit()?;
        Ok(n)
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
    #[test] fn open_uses_wal_and_full_sync() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("lib.db")).unwrap();
        let sync: i64 = db.conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        assert_eq!(sync, 2); // FULL
        db.set_bulk(true).unwrap();
        let sync: i64 = db.conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        assert_eq!(sync, 1); // NORMAL
        db.set_bulk(false).unwrap();
        let sync: i64 = db.conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        assert_eq!(sync, 2);
    }
    #[test] fn song_counts() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.add_root("D:/m").unwrap();
        let r2 = db.add_root("D:/n").unwrap();
        let three: Vec<_> = (0..3).map(|i| song("D:/m", "a.flac", i * 1000, i as u32 + 1)).collect();
        db.replace_file(r.id, &meta("a.flac"), &three).unwrap();
        assert_eq!(db.song_count().unwrap(), 3);
        let c = db.root_song_counts().unwrap();
        assert_eq!(c.get(&r.id), Some(&3));
        assert_eq!(c.get(&r2.id), None);
    }
    #[test] fn migration_failure_rolls_back() {
        let mut c = rusqlite::Connection::open_in_memory().unwrap();
        let bad: &[&str] = &["CREATE TABLE y(a);", "CREATE TABLE x(a); CREATE TABLE y(a);"];
        assert!(migrate_with(&mut c, bad).is_err());
        let n: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name='x'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0); // v2 rolled back entirely
        assert_eq!(schema_version(&c).unwrap(), 1); // v1 kept
    }

    // ---- playlists / favorites ----
    const ROOT: &str = "D:/m";
    /// Insert a one-song file; returns the song id.
    fn mk(db: &mut Db, rel: &str, title: &str, artist: &str, size: u64, dur: f64) -> String {
        let r = db.add_root(ROOT).unwrap();
        let mut m = meta(rel); m.size = size; m.duration = dur;
        let mut s = song(ROOT, rel, 0, 1); s.title = title.into(); s.artist = artist.into(); s.duration = dur;
        db.replace_file(r.id, &m, &[s.clone()]).unwrap();
        s.id
    }
    fn ids(items: &[PlItem]) -> Vec<String> { items.iter().map(|i| i.song_id.clone()).collect() }
    /// A, B, C, D with distinct sizes.
    fn four(db: &mut Db) -> Vec<String> {
        vec![mk(db, "a.mp3", "A", "x", 101, 10.0), mk(db, "b.mp3", "B", "x", 102, 20.0),
             mk(db, "c.mp3", "C", "x", 103, 30.0), mk(db, "d.mp3", "D", "x", 104, 40.0)]
    }

    #[test] fn migrates_v1_to_v2() {
        let mut c = rusqlite::Connection::open_in_memory().unwrap();
        migrate_with(&mut c, &MIGRATIONS[..1]).unwrap();
        c.execute("INSERT INTO roots(path) VALUES('D:/m')", []).unwrap();
        c.execute("INSERT INTO files(root_id,rel,ext,size,mtime,codec,bitrate,duration) VALUES(1,'a.mp3','mp3',1,1,'mp3',1,1.0)", []).unwrap();
        c.execute("INSERT INTO songs VALUES('s1',1,'t','a','b',1,0,NULL,1.0)", []).unwrap();
        assert_eq!(schema_version(&c).unwrap(), 1);
        let db = Db::init(c).unwrap();
        assert_eq!(schema_version(db.conn()).unwrap(), 2);
        assert_eq!(db.song_count().unwrap(), 1);
        assert!(db.playlists().unwrap().is_empty());
    }
    #[test] fn playlist_crud() {
        let db = Db::open_in_memory().unwrap();
        let id = db.create_playlist("晴天", 10).unwrap();
        assert!(db.playlist_exists(id).unwrap());
        assert!(db.rename_playlist(id, "改名", 20).unwrap());
        let p = db.playlists().unwrap();
        assert_eq!(p[0].name, "改名"); assert_eq!(p[0].created, 10); assert_eq!(p[0].updated, 20);
        assert!(!db.rename_playlist(999, "x", 1).unwrap());
        assert!(db.delete_playlist(id).unwrap());
        assert!(!db.delete_playlist(id).unwrap());
        assert!(!db.playlist_exists(id).unwrap());
    }
    #[test] fn add_skips_duplicates_keeps_order() {
        let mut db = Db::open_in_memory().unwrap();
        let s = four(&mut db);
        let (a, b, c) = (s[0].clone(), s[1].clone(), s[2].clone());
        let pl = db.create_playlist("p", 1).unwrap();
        db.playlist_add(pl, &[b.clone()], 2).unwrap();
        let out = db.playlist_add(pl, &[a.clone(), b.clone(), c.clone(), a.clone(), "nope".into()], 3).unwrap();
        assert_eq!(out, AddOutcome { added: 2, duplicate: 2, unknown: 1 });
        let items = db.playlist_items(pl).unwrap();
        assert_eq!(ids(&items), vec![b.clone(), a.clone(), c.clone()]);
        let ai = &items[1];
        assert_eq!(ai.snap, Snapshot { title: "A".into(), artist: "x".into(), album: "b".into(), duration: 10.0, size: 101 });
        assert_eq!(ai.added, 3);
        let row = &db.playlists().unwrap()[0];
        assert_eq!(row.count, 3); assert_eq!(row.duration, 60.0); assert_eq!(row.updated, 3);
    }
    #[test] fn remove_and_move() {
        let mut db = Db::open_in_memory().unwrap();
        let s = four(&mut db);
        let pl = db.create_playlist("p", 1).unwrap();
        db.playlist_add(pl, &s, 2).unwrap();
        let item = |db: &Db, i: usize| db.playlist_items(pl).unwrap()[i].item_id;
        let d = item(&db, 3);
        assert!(db.playlist_move(pl, d, 0).unwrap());
        assert_eq!(ids(&db.playlist_items(pl).unwrap()), vec![s[3].clone(), s[0].clone(), s[1].clone(), s[2].clone()]);
        let a = item(&db, 1);
        assert!(db.playlist_move(pl, a, 99).unwrap());
        assert_eq!(ids(&db.playlist_items(pl).unwrap()), vec![s[3].clone(), s[1].clone(), s[2].clone(), s[0].clone()]);
        let rm = vec![item(&db, 1), item(&db, 2)];
        assert_eq!(db.playlist_remove(pl, &rm).unwrap(), 2);
        assert_eq!(ids(&db.playlist_items(pl).unwrap()), vec![s[3].clone(), s[0].clone()]);
        assert!(!db.playlist_move(pl, 9999, 0).unwrap());
    }
    #[test] fn items_survive_song_deletion() {
        let mut db = Db::open_in_memory().unwrap();
        let a = mk(&mut db, "a.mp3", "A", "x", 101, 10.0);
        let r2 = db.add_root("D:/n").unwrap();
        let mut m = meta("b.mp3"); m.size = 102;
        let mut sb = song("D:/n", "b.mp3", 0, 2); sb.title = "B".into();
        db.replace_file(r2.id, &m, &[sb.clone()]).unwrap();
        let pl = db.create_playlist("p", 1).unwrap();
        db.playlist_add(pl, &[a.clone(), sb.id.clone()], 2).unwrap();
        db.set_favorites(&[a.clone(), sb.id.clone()], true, 3).unwrap();
        let root = db.roots().unwrap()[0].id;
        db.delete_files(root, &["a.mp3".to_string()]).unwrap();
        db.remove_root(r2.id).unwrap();
        assert_eq!(db.song_count().unwrap(), 0);
        let items = db.playlist_items(pl).unwrap();
        assert_eq!(ids(&items), vec![a.clone(), sb.id.clone()]);
        assert_eq!(items[0].snap.title, "A"); assert_eq!(items[0].snap.size, 101);
        assert_eq!(items[1].snap.title, "B");
        assert_eq!(db.favorites().unwrap().len(), 2);
    }
    #[test] fn favorites_order_and_toggle() {
        let mut db = Db::open_in_memory().unwrap();
        let s = four(&mut db);
        assert_eq!(db.set_favorites(&[s[0].clone()], true, 1).unwrap(), 1);
        assert_eq!(db.set_favorites(&[s[1].clone()], true, 2).unwrap(), 1);
        let f = db.favorites().unwrap();
        assert_eq!(f.iter().map(|r| r.song_id.clone()).collect::<Vec<_>>(), vec![s[1].clone(), s[0].clone()]);
        assert_eq!(db.set_favorites(&[s[0].clone()], true, 5).unwrap(), 0);
        assert_eq!(db.favorites().unwrap()[1].added, 1);
        assert_eq!(db.set_favorites(&[s[0].clone(), "nope".into()], false, 6).unwrap(), 1);
        assert_eq!(db.favorites().unwrap().len(), 1);
    }
    /// A (to be lost) with a playlist item and favorite, plus helper to add renamed copies.
    fn relink_setup(db: &mut Db) -> (String, i64) {
        let a = mk(db, "a.mp3", "Yesterday", "The Beatles", 500, 100.0);
        let pl = db.create_playlist("p", 1).unwrap();
        db.playlist_add(pl, &[a.clone()], 2).unwrap();
        db.set_favorites(&[a.clone()], true, 3).unwrap();
        let root = db.roots().unwrap()[0].id;
        db.delete_files(root, &["a.mp3".to_string()]).unwrap();
        (a, pl)
    }
    #[test] fn relink_moves_item_to_renamed_file() {
        let mut db = Db::open_in_memory().unwrap();
        let (_a, pl) = relink_setup(&mut db);
        let a2 = mk(&mut db, "new/a.mp3", " yesterday ", "THE BEATLES ", 500, 100.4);
        assert_eq!(db.relink_missing().unwrap(), 2);
        let items = db.playlist_items(pl).unwrap();
        assert_eq!(ids(&items), vec![a2.clone()]);
        assert_eq!(items[0].snap.title, " yesterday "); assert_eq!(items[0].snap.duration, 100.4);
        let f = db.favorites().unwrap();
        assert_eq!(f.len(), 1); assert_eq!(f[0].song_id, a2); assert_eq!(f[0].snap.artist, "THE BEATLES ");
        assert_eq!(f[0].added, 3);
        assert_eq!(db.relink_missing().unwrap(), 0);
    }
    #[test] fn relink_skips_ambiguous() {
        let mut db = Db::open_in_memory().unwrap();
        let (a, pl) = relink_setup(&mut db);
        mk(&mut db, "n1.mp3", "Yesterday", "The Beatles", 500, 100.0);
        mk(&mut db, "n2.mp3", "Yesterday", "The Beatles", 500, 100.2);
        assert_eq!(db.relink_missing().unwrap(), 0);
        assert_eq!(ids(&db.playlist_items(pl).unwrap()), vec![a.clone()]);
        assert_eq!(db.favorites().unwrap()[0].song_id, a);
    }
    #[test] fn relink_merges_into_existing() {
        let mut db = Db::open_in_memory().unwrap();
        let (_a, pl) = relink_setup(&mut db);
        let a2 = mk(&mut db, "new.mp3", "Yesterday", "The Beatles", 500, 100.0);
        db.playlist_add(pl, &[a2.clone()], 4).unwrap();
        db.set_favorites(&[a2.clone()], true, 5).unwrap();
        assert_eq!(db.relink_missing().unwrap(), 2);
        assert_eq!(ids(&db.playlist_items(pl).unwrap()), vec![a2.clone()]);
        let f = db.favorites().unwrap();
        assert_eq!(f.len(), 1); assert_eq!(f[0].song_id, a2);
    }
    #[test] fn relink_ignores_songs_of_offline_roots() {
        let mut db = Db::open_in_memory().unwrap();
        let a = mk(&mut db, "a.mp3", "Yesterday", "The Beatles", 500, 100.0);
        db.set_favorites(&[a.clone()], true, 1).unwrap();
        mk(&mut db, "b.mp3", "Yesterday", "The Beatles", 500, 100.0);
        assert_eq!(db.relink_missing().unwrap(), 0);
        assert_eq!(db.favorites().unwrap()[0].song_id, a);
    }
    #[test] fn relink_many_missing_is_fast() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.add_root(ROOT).unwrap();
        for i in 0..10000u32 {
            let rel = format!("f{i}.mp3");
            let mut m = meta(&rel); m.size = 1000 + i as u64;
            db.replace_file(r.id, &m, &[song(ROOT, &rel, 0, i)]).unwrap();
        }
        let pl = db.create_playlist("p", 1).unwrap();
        {
            let tx = db.conn.transaction().unwrap();
            for i in 0..2000u32 {
                // even: matches library song f{i} (title t{i}, size 1000+i); odd: no match
                let size = if i % 2 == 0 { 1000 + i as i64 } else { 900_000 + i as i64 };
                tx.execute(
                    "INSERT INTO playlist_items(playlist_id,pos,song_id,title,artist,album,duration,size,added)
                     VALUES(?1,?2,?3,?4,'a','b',1.5,?5,1)",
                    params![pl, i, format!("gone{i}"), format!("T{i} "), size]).unwrap();
            }
            tx.commit().unwrap();
        }
        let t = std::time::Instant::now();
        assert_eq!(db.relink_missing().unwrap(), 1000);
        assert!(t.elapsed().as_secs_f64() < 1.0, "{:?}", t.elapsed());
        let items = db.playlist_items(pl).unwrap();
        assert_eq!(items[0].song_id, song_id(ROOT, "f0.mp3", 0));
        assert_eq!(items[1].song_id, "gone1");
        let t = std::time::Instant::now();
        assert_eq!(db.relink_missing().unwrap(), 0);
        assert!(t.elapsed().as_secs_f64() < 1.0, "{:?}", t.elapsed());
    }
    #[test] fn saved_ids_and_memberships() {
        let mut db = Db::open_in_memory().unwrap();
        let s = four(&mut db);
        let (a, b, c, d) = (s[0].clone(), s[1].clone(), s[2].clone(), s[3].clone());
        let p1 = db.create_playlist("1", 1).unwrap();
        let p2 = db.create_playlist("2", 2).unwrap();
        db.set_favorites(&[a.clone()], true, 1).unwrap();
        db.playlist_add(p1, &[b.clone(), c.clone()], 1).unwrap();
        db.playlist_add(p2, &[c.clone()], 1).unwrap();
        let mut saved = db.saved_song_ids().unwrap(); saved.sort();
        let mut want = vec![a.clone(), b.clone(), c.clone()]; want.sort();
        assert_eq!(saved, want);
        assert_eq!(db.memberships(&[a.clone(), b.clone(), c.clone()]).unwrap(), (1, vec![(p1, 2), (p2, 1)]));
        assert_eq!(db.memberships(&[d]).unwrap(), (0, vec![(p1, 0), (p2, 0)]));
    }
    #[test] fn save_songs_adds_and_removes_atomically() {
        let mut db = Db::open_in_memory().unwrap();
        let s = four(&mut db);
        let (a, b, c) = (s[0].clone(), s[1].clone(), s[2].clone());
        let p1 = db.create_playlist("1", 1).unwrap();
        let p2 = db.create_playlist("2", 2).unwrap();
        db.playlist_add(p1, &[c.clone(), a.clone()], 1).unwrap();
        db.playlist_add(p2, &[a.clone(), b.clone(), c.clone()], 1).unwrap();
        let r = db.save_songs(&[a.clone(), b.clone()], &[0, p1], &[p2], 9).unwrap();
        // adds: fav a,b (2 new) + p1 b new, a existing => 3 added, 1 existing; removed 2 from p2
        assert_eq!(r, (3, 1, 2));
        let fav: Vec<_> = db.favorites().unwrap().into_iter().map(|f| f.song_id).collect();
        assert_eq!(fav.len(), 2); assert!(fav.contains(&a) && fav.contains(&b));
        assert_eq!(ids(&db.playlist_items(p1).unwrap()), vec![c.clone(), a.clone(), b.clone()]);
        assert_eq!(ids(&db.playlist_items(p2).unwrap()), vec![c.clone()]);
        // unknown playlist in add or remove: Err + full rollback
        assert!(db.save_songs(&[c.clone()], &[0, 999], &[], 10).is_err());
        assert!(db.save_songs(&[c.clone()], &[0], &[999], 10).is_err());
        let fav: Vec<_> = db.favorites().unwrap().into_iter().map(|f| f.song_id).collect();
        assert_eq!(fav.len(), 2); assert!(!fav.contains(&c));
        assert_eq!(db.playlist_items(p2).unwrap().len(), 1);
    }
    #[test] fn save_songs_remove_from_favorites() {
        let mut db = Db::open_in_memory().unwrap();
        let s = four(&mut db);
        db.set_favorites(&[s[0].clone()], true, 1).unwrap();
        let r = db.save_songs(&[s[0].clone()], &[], &[0], 2).unwrap();
        assert_eq!(r, (0, 0, 1));
        assert!(db.favorites().unwrap().is_empty());
    }
}
