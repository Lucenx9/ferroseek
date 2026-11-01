// SPDX-License-Identifier: GPL-3.0-or-later
//! A small pool of read-only index connections.
//!
//! Readers are reused between launcher queries (warm SQLite page cache) and
//! dropped when `Release` clears the pool.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Mutex;

use ferroseek_index::{Reader, Result};

/// At most this many idle readers are kept; concurrent queries beyond it
/// open temporary connections (~0.2 ms each).
const MAX_IDLE: usize = 4;

pub struct Pool {
    db: PathBuf,
    idle: Mutex<Idle>,
}

#[derive(Default)]
struct Idle {
    readers: Vec<Reader>,
    epoch: u64,
    identity: Option<(u64, u64)>,
}

impl Pool {
    pub fn new(db: PathBuf) -> Pool {
        Pool { db, idle: Mutex::new(Idle::default()) }
    }

    /// Runs `f` with a reader. A reader that hit an error is discarded.
    pub fn with<T>(&self, f: impl FnOnce(&Reader) -> Result<T>) -> Result<T> {
        let identity = std::fs::metadata(&self.db).map(|m| (m.dev(), m.ino()))?;
        let (r, epoch) = {
            let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
            if idle.identity != Some(identity) {
                idle.readers.clear();
                idle.epoch = idle.epoch.wrapping_add(1);
                idle.identity = Some(identity);
            }
            (idle.readers.pop(), idle.epoch)
        };
        let r = match r {
            Some(r) => r,
            None => Reader::open(&self.db)?,
        };
        let out = f(&r);
        if out.is_ok() {
            let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
            if idle.epoch == epoch && idle.readers.len() < MAX_IDLE {
                idle.readers.push(r);
            }
        }
        out
    }

    pub fn clear(&self) {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        idle.epoch = idle.epoch.wrapping_add(1);
        idle.readers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_does_not_recapture_inflight_reader() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("index");
        let (w, _) = ferroseek_index::Writer::open(&db).unwrap();
        let pool = Pool::new(db);
        pool.with(|r| {
            pool.clear();
            r.stats()
        })
        .unwrap();
        assert!(pool.idle.lock().unwrap().readers.is_empty());
        w.close().unwrap();
    }
    #[test]
    fn replacement_database_invalidates_idle_readers() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("index");
        let (w, _) = ferroseek_index::Writer::open(&db).unwrap();
        w.close().unwrap();
        let pool = Pool::new(db.clone());
        pool.with(|r| r.stats()).unwrap();
        let replacement = t.path().join("replacement");
        let (mut w, _) = ferroseek_index::Writer::open(&replacement).unwrap();
        let mut tx = w.transaction().unwrap();
        tx.ensure_root(
            std::path::Path::new("/new"),
            ferroseek_index::Meta { kind: ferroseek_index::Kind::Dir, size: 0, mtime_ns: 0, ino: 1 },
        )
        .unwrap();
        tx.commit().unwrap();
        w.close().unwrap();
        std::fs::rename(&replacement, &db).unwrap();
        assert_eq!(pool.with(|r| r.stats()).unwrap().roots, vec![PathBuf::from("/new")]);
    }
}
