// SPDX-License-Identifier: GPL-3.0-or-later
//! `ferroseekd`: the FerroSeek indexing daemon.
//!
//! Threads: main (waits for SIGTERM/SIGINT), zbus helper (D-Bus), the
//! engine's inotify reader and indexer, and short-lived blocking-pool
//! threads for queries (they exit after 10 s idle).

mod control;
mod mime;
mod open;
mod pool;
mod runner;

use std::process::ExitCode;
use std::sync::Arc;

use ferroseek_engine::{Config, Engine, EngineError, Paths, ipc};

const HELP: &str = "\
ferroseekd — FerroSeek file indexing daemon

USAGE: ferroseekd [--no-dbus] [--version] [--help]

Indexes the roots from $XDG_CONFIG_HOME/ferroseek/config.toml, keeps the
index current with inotify, and serves the KRunner runner and the control
interface on the session bus as io.github.ferroseek.FerroSeek.

  --no-dbus   Run the indexer only (no session bus), e.g. for benchmarks.

Environment: FERROSEEK_LOG (error|warn|info|debug), FERROSEEK_CONFIG,
FERROSEEK_DATA_DIR.";

/// Blocks SIGTERM/SIGINT/SIGHUP in the calling thread. Done first in
/// `main`, before any thread exists, so every thread inherits the mask and
/// only `sigwait` in main receives them.
fn block_signals() -> libc::sigset_t {
    // SAFETY: sigset manipulation on a local, then pthread_sigmask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

fn wait_for_signal(set: &libc::sigset_t) -> i32 {
    let mut sig = 0;
    // SAFETY: valid set and out pointer.
    unsafe { libc::sigwait(set, &mut sig) };
    sig
}

fn main() -> ExitCode {
    let mut use_dbus = true;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--no-dbus" => use_dbus = false,
            "--version" | "-V" => {
                println!("ferroseekd {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("ferroseekd: unknown argument {other:?}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }
    let sigs = block_signals();
    // Few threads ever allocate; without a cap glibc gives each short-lived
    // query thread its own arena, whose freed pages then linger as RSS.
    #[cfg(target_env = "gnu")]
    // SAFETY: mallopt before any other thread exists.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    ferroseek_engine::logging::init();

    let paths = Paths::from_env();
    let cfg = match Config::load(&paths.config_file) {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::from(2);
        }
    };
    let engine = match Engine::start(&cfg, &paths) {
        Ok(e) => Arc::new(e),
        Err(EngineError::Busy) => {
            log::error!("another FerroSeek daemon or indexer is already running for {}", paths.data_dir.display());
            return ExitCode::from(3);
        }
        Err(e) => {
            log::error!("cannot start engine: {e}");
            return ExitCode::from(1);
        }
    };
    log::info!(
        "ferroseekd {} indexing {} into {}",
        env!("CARGO_PKG_VERSION"),
        cfg.root_paths().iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
        paths.index_file().display()
    );

    // Kept alive for the life of the process; zbus serves it on its helper
    // thread after block_on returns.
    let _conn = if use_dbus {
        let pool = Arc::new(pool::Pool::new(paths.index_file()));
        let control = control::Control { engine: engine.clone(), paths: paths.clone() };
        let runner = runner::Runner::new(pool);
        let built = zbus::block_on(async {
            zbus::connection::Builder::session()
                .name(ipc::BUS_NAME)
                .serve_at(ipc::CONTROL_PATH, control)
                .serve_at(ipc::RUNNER_PATH, runner)
                .build()
                .await
        });
        match built {
            Ok(c) => Some(c),
            Err(e) => {
                log::error!("cannot register {} on the session bus: {e}", ipc::BUS_NAME);
                engine.shutdown();
                return ExitCode::from(4);
            }
        }
    } else {
        None
    };

    let sig = wait_for_signal(&sigs);
    log::info!("received signal {sig}; shutting down");
    engine.shutdown();
    ExitCode::SUCCESS
}
