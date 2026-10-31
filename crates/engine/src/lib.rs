// SPDX-License-Identifier: GPL-3.0-or-later
//! FerroSeek indexing engine.
//!
//! [`Engine`] runs the long-lived pipeline (inotify reader thread + indexer
//! thread owning the index writer); [`index_once`] is the one-shot variant.
//! Configuration, XDG paths and exclusion rules live in [`config`].

pub mod config;
mod engine;
mod indexer;
pub mod logging;
pub mod inotify;
pub mod status;
pub mod sys;

pub use config::{Config, Paths, Rules};
pub use engine::{Engine, EngineError, index_once};
pub use status::{Snapshot, State, Status};

/// D-Bus names shared by the daemon (server) and the CLI (client).
pub mod ipc {
    pub const BUS_NAME: &str = "io.github.ferroseek.FerroSeek";
    pub const CONTROL_PATH: &str = "/io/github/ferroseek/FerroSeek";
    pub const CONTROL_IFACE: &str = "io.github.ferroseek.FerroSeek1";
    pub const RUNNER_PATH: &str = "/runner";
}
