// SPDX-License-Identifier: GPL-3.0-or-later
//! A small pool of read-only index connections.
//!
//! Readers are reused within a KRunner session (warm SQLite page cache) and
//! dropped on `Teardown`, so an idle daemon holds no reader connections.

use std::path::PathBuf;
use std::sync::Mutex;

use ferroseek_index::{Reader, Result};

/// At most this many idle readers are kept; concurrent queries beyond it
/// open temporary connections (~0.2 ms each).
const MAX_IDLE: usize = 4;

pub struct Pool {
    db: PathBuf,
    idle: Mutex<Vec<Reader>>,
}

impl Pool {
    pub fn new(db: PathBuf) -> Pool {
        Pool { db, idle: Mutex::new(Vec::new()) }
    }

    /// Runs `f` with a reader. A reader that hit an error is discarded.
    pub fn with<T>(&self, f: impl FnOnce(&Reader) -> Result<T>) -> Result<T> {
        let r = self.idle.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let r = match r {
            Some(r) => r,
            None => Reader::open(&self.db)?,
        };
        let out = f(&r);
        if out.is_ok() {
            let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
            if idle.len() < MAX_IDLE {
                idle.push(r);
            }
        }
        out
    }

    pub fn clear(&self) {
        self.idle.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}
