//! Shared application state managed by Tauri.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, RwLock};

use crate::db::Db;

pub struct AppState {
    pub db: Mutex<Db>,
    pub data_dir: PathBuf,
    /// ids of roots currently reachable (optimistically all roots until probed)
    pub online: RwLock<HashSet<i64>>,
    pub scanning: AtomicBool,
}

impl AppState {
    pub fn new(db: Db, data_dir: PathBuf) -> AppState {
        let online = db.roots().unwrap_or_default().into_iter().map(|r| r.id).collect();
        AppState { db: Mutex::new(db), data_dir, online: RwLock::new(online), scanning: AtomicBool::new(false) }
    }
    /// Locks the database, recovering from a poisoned lock.
    pub fn db(&self) -> MutexGuard<'_, Db> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn online(&self) -> HashSet<i64> {
        self.online.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set_online(&self, ids: HashSet<i64>) {
        *self.online.write().unwrap_or_else(|e| e.into_inner()) = ids;
    }
}
