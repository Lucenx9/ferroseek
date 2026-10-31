// SPDX-License-Identifier: GPL-3.0-or-later
//! Observable engine state, shared lock-free between the indexer thread and
//! status readers (D-Bus, CLI).

use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting = 0,
    Idle = 1,
    Scanning = 2,
    Paused = 3,
    Stopping = 4,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Idle => "idle",
            State::Scanning => "scanning",
            State::Paused => "paused",
            State::Stopping => "stopping",
        }
    }

    fn from_u8(v: u8) -> State {
        match v {
            1 => State::Idle,
            2 => State::Scanning,
            3 => State::Paused,
            4 => State::Stopping,
            _ => State::Starting,
        }
    }
}

#[derive(Default, Debug)]
pub struct Status {
    state: AtomicU8,
    pub queued_dirs: AtomicU64,
    pub scanned_dirs: AtomicU64,
    pub watched_dirs: AtomicU64,
    pub watch_limit_hit: AtomicBool,
    pub events: AtomicU64,
    pub overflows: AtomicU64,
    pub errors: AtomicU64,
    /// Unix seconds when the last full reconcile pass finished.
    pub last_pass_end: AtomicI64,
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub disk_bytes: AtomicU64,
}

impl Status {
    pub fn state(&self) -> State {
        State::from_u8(self.state.load(Relaxed))
    }

    pub fn set_state(&self, s: State) {
        self.state.store(s as u8, Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state().as_str().to_string(),
            files: self.files.load(Relaxed),
            dirs: self.dirs.load(Relaxed),
            disk_bytes: self.disk_bytes.load(Relaxed),
            queued_dirs: self.queued_dirs.load(Relaxed),
            scanned_dirs: self.scanned_dirs.load(Relaxed),
            watched_dirs: self.watched_dirs.load(Relaxed),
            watch_limit_hit: self.watch_limit_hit.load(Relaxed),
            events: self.events.load(Relaxed),
            overflows: self.overflows.load(Relaxed),
            errors: self.errors.load(Relaxed),
            last_pass_end: self.last_pass_end.load(Relaxed),
        }
    }
}

/// Plain copy of [`Status`] for reporting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub state: String,
    pub files: u64,
    pub dirs: u64,
    pub disk_bytes: u64,
    pub queued_dirs: u64,
    pub scanned_dirs: u64,
    pub watched_dirs: u64,
    pub watch_limit_hit: bool,
    pub events: u64,
    pub overflows: u64,
    pub errors: u64,
    pub last_pass_end: i64,
}

impl Snapshot {
    /// `key=value` lines; also the D-Bus wire format (as a dict).
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("state", self.state.clone()),
            ("files", self.files.to_string()),
            ("dirs", self.dirs.to_string()),
            ("disk_bytes", self.disk_bytes.to_string()),
            ("queued_dirs", self.queued_dirs.to_string()),
            ("scanned_dirs", self.scanned_dirs.to_string()),
            ("watched_dirs", self.watched_dirs.to_string()),
            ("watch_limit_hit", self.watch_limit_hit.to_string()),
            ("events", self.events.to_string()),
            ("overflows", self.overflows.to_string()),
            ("errors", self.errors.to_string()),
            ("last_pass_end", self.last_pass_end.to_string()),
        ]
    }
}
