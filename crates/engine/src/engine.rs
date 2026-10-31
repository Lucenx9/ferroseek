// SPDX-License-Identifier: GPL-3.0-or-later
//! Threads and scheduling around the [`Indexer`].

use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ferroseek_index::{OpenOutcome, Writer};

use crate::config::{Config, Paths, Rules};
use crate::indexer::{Indexer, Mode};
use crate::inotify::{Inotify, RawEvent};
use crate::status::{Snapshot, State, Status};
use crate::sys::{InstanceLock, WakePipe, lower_current_thread_priority, poll2};

#[derive(Debug)]
pub enum EngineError {
    /// Another process (daemon or one-shot indexer) owns this index.
    Busy,
    Io(std::io::Error),
    Index(ferroseek_index::Error),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Busy => write!(f, "another FerroSeek process is already indexing this index"),
            EngineError::Io(e) => write!(f, "{e}"),
            EngineError::Index(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e)
    }
}

impl From<ferroseek_index::Error> for EngineError {
    fn from(e: ferroseek_index::Error) -> Self {
        EngineError::Index(e)
    }
}

enum Msg {
    Events(Vec<RawEvent>),
    Pause,
    Resume,
    Reindex(Mode),
    Rebuild,
    Stop,
}

/// Bound on events held while debouncing; beyond it the batch is dropped
/// and a reconcile is scheduled instead (memory stays bounded for any
/// burst).
const MAX_PENDING_EVENTS: usize = 100_000;
/// Channel capacity in *read batches* (each up to 64 KiB of events).
const CHANNEL_BATCHES: usize = 64;

/// A running indexing engine: an inotify reader thread and an indexer
/// thread that owns the index writer.
pub struct Engine {
    tx: SyncSender<Msg>,
    status: Arc<Status>,
    indexer: Mutex<Option<JoinHandle<()>>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    stop_reader: Arc<AtomicBool>,
    wake: Arc<WakePipe>,
    lock: Mutex<Option<InstanceLock>>,
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn open_writer(paths: &Paths) -> Result<Writer, EngineError> {
    let (w, outcome) = Writer::open(&paths.index_file())?;
    match outcome {
        OpenOutcome::Created => log::info!("created index {}", paths.index_file().display()),
        OpenOutcome::Recovered { reason, moved_to } => {
            log::warn!("index was unusable ({reason}); old file kept at {}; rebuilding", moved_to.display())
        }
        OpenOutcome::Existing => {}
    }
    Ok(w)
}

fn acquire(paths: &Paths) -> Result<InstanceLock, EngineError> {
    paths.ensure_data_dir()?;
    InstanceLock::try_acquire(&paths.lock_file())?.ok_or(EngineError::Busy)
}

fn rules(cfg: &Config, paths: &Paths) -> Rules {
    Rules::new(cfg, &[paths.data_dir.clone()])
}

fn refresh_counts(ix: &Indexer, status: &Status) {
    if let Ok(s) = ix.writer().stats() {
        status.files.store(s.files, Relaxed);
        status.dirs.store(s.dirs, Relaxed);
        status.disk_bytes.store(s.disk_bytes, Relaxed);
    }
}

/// Indexes the configured roots once, without watching, then returns. For
/// `ferroseek index` when no daemon is running.
pub fn index_once(cfg: &Config, paths: &Paths, full: bool) -> Result<Snapshot, EngineError> {
    let _lock = acquire(paths)?;
    let status = Arc::new(Status::default());
    let mut ix = Indexer::new(open_writer(paths)?, rules(cfg, paths), cfg.root_paths(), None, status.clone());
    status.set_state(State::Scanning);
    ix.sync_roots(if full { Mode::Full } else { Mode::Quick })?;
    ix.scan_to_completion()?;
    status.last_pass_end.store(now_unix(), Relaxed);
    refresh_counts(&ix, &status);
    ix.into_writer().close()?;
    status.set_state(State::Idle);
    Ok(status.snapshot())
}

impl Engine {
    /// Opens (or recovers) the index, starts watching and queues a quick
    /// reconcile of all roots. Fails with [`EngineError::Busy`] if another
    /// process owns the index.
    pub fn start(cfg: &Config, paths: &Paths) -> Result<Engine, EngineError> {
        let lock = acquire(paths)?;
        let status = Arc::new(Status::default());
        let ino = Inotify::new()?;
        let ix = Indexer::new(open_writer(paths)?, rules(cfg, paths), cfg.root_paths(), Some(ino.clone()), status.clone());
        let (tx, rx) = mpsc::sync_channel::<Msg>(CHANNEL_BATCHES);

        let wake = Arc::new(WakePipe::new()?);
        let stop_reader = Arc::new(AtomicBool::new(false));
        let reader = {
            let (tx, wake, stop) = (tx.clone(), wake.clone(), stop_reader.clone());
            std::thread::Builder::new().name("fs-inotify".into()).spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match poll2(ino.raw_fd(), wake.read.as_raw_fd()) {
                        Ok((_, true)) | Err(_) => return,
                        Ok(_) => {}
                    }
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    let mut evs = Vec::new();
                    if let Err(e) = ino.read_available(&mut buf, &mut evs) {
                        log::error!("inotify read failed: {e}");
                        return;
                    }
                    // Blocks when the indexer is behind: backpressure into
                    // the kernel queue, which overflows into a reconcile.
                    if !evs.is_empty() && tx.send(Msg::Events(evs)).is_err() {
                        return;
                    }
                }
            })?
        };

        let cfg = cfg.clone();
        let paths = paths.clone();
        let st = status.clone();
        let indexer = std::thread::Builder::new()
            .name("fs-indexer".into())
            .spawn(move || {
                if cfg.low_priority {
                    lower_current_thread_priority();
                }
                run_indexer(ix, rx, &cfg, &paths, st);
            })?;
        Ok(Engine {
            tx,
            status,
            indexer: Mutex::new(Some(indexer)),
            reader: Mutex::new(Some(reader)),
            stop_reader,
            wake,
            lock: Mutex::new(Some(lock)),
        })
    }

    pub fn status(&self) -> Snapshot {
        self.status.snapshot()
    }

    pub fn status_handle(&self) -> Arc<Status> {
        self.status.clone()
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Msg::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Msg::Resume);
    }

    /// Queues a reconcile of all roots (`full`: re-stat every file).
    pub fn reindex(&self, full: bool) {
        let _ = self.tx.send(Msg::Reindex(if full { Mode::Full } else { Mode::Quick }));
    }

    /// Drops the whole index and rescans.
    pub fn rebuild(&self) {
        let _ = self.tx.send(Msg::Rebuild);
    }

    /// Polls until the engine is idle with no queued work, or `timeout`.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            let s = self.status.snapshot();
            if s.state == "idle" && s.queued_dirs == 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Stops both threads, committing and closing the index cleanly.
    /// Idempotent; later commands are ignored.
    pub fn shutdown(&self) {
        self.stop_reader.store(true, Ordering::Release);
        self.wake.wake();
        if let Some(r) = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = r.join();
        }
        let _ = self.tx.send(Msg::Stop);
        if let Some(i) = self.indexer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = i.join();
        }
        // The index is closed; another process may take over now.
        self.lock.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_indexer(mut ix: Indexer, rx: mpsc::Receiver<Msg>, cfg: &Config, paths: &Paths, status: Arc<Status>) {
    let status = &status;
    let debounce = Duration::from_millis(cfg.debounce_ms);
    let periodic = (cfg.reconcile_interval_minutes > 0).then(|| Duration::from_secs(cfg.reconcile_interval_minutes * 60));
    let duty = cfg.max_cpu_percent.clamp(1, 100);
    let mut next_periodic = periodic.map(|p| Instant::now() + p);
    let mut paused = false;
    let mut missed_while_paused = false;
    let mut pending: Vec<RawEvent> = Vec::new();
    let mut first_event: Option<Instant> = None;
    let mut last_event = Instant::now();
    let mut scan_not_before = Instant::now();
    let mut scanning = true;

    status.set_state(State::Scanning);
    if let Err(e) = ix.sync_roots(Mode::Quick) {
        log::error!("initial reconcile failed: {e}");
    }

    loop {
        let now = Instant::now();
        let mut timeout = Duration::from_secs(3600);
        if !paused {
            if ix.has_work() {
                timeout = timeout.min(scan_not_before.saturating_duration_since(now));
            }
            if let Some(t) = first_event {
                timeout = timeout.min((t + debounce).saturating_duration_since(now));
            }
            if ix.has_retries() {
                timeout = timeout.min(Duration::from_secs(1));
            }
            if let Some(p) = next_periodic {
                timeout = timeout.min(p.saturating_duration_since(now));
            }
        }
        let msg = match rx.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let result: Result<bool, ferroseek_index::Error> = (|| {
            match msg {
                Some(Msg::Events(evs)) => {
                    last_event = Instant::now();
                    if paused {
                        missed_while_paused = true;
                    } else if pending.len() + evs.len() > MAX_PENDING_EVENTS {
                        log::warn!("event burst exceeded {MAX_PENDING_EVENTS} pending events; reconciling instead");
                        pending.clear();
                        first_event = None;
                        ix.schedule_all(Mode::Quick)?;
                    } else {
                        pending.extend(evs);
                        first_event.get_or_insert_with(Instant::now);
                    }
                }
                Some(Msg::Pause) => {
                    paused = true;
                    status.set_state(State::Paused);
                }
                Some(Msg::Resume) => {
                    if paused {
                        paused = false;
                        // Events were dropped while paused; a quick pass
                        // restores consistency.
                        if missed_while_paused || !pending.is_empty() || ix.has_work() {
                            pending.clear();
                            first_event = None;
                            ix.schedule_all(Mode::Quick)?;
                        }
                        missed_while_paused = false;
                    }
                }
                Some(Msg::Reindex(mode)) => ix.schedule_all(mode)?,
                Some(Msg::Rebuild) => {
                    pending.clear();
                    first_event = None;
                    ix.rebuild()?;
                }
                Some(Msg::Stop) => return Ok(true),
                None => {}
            }
            if paused {
                return Ok(false);
            }
            let now = Instant::now();
            if let Some(t) = first_event {
                if now >= t + debounce || pending.len() >= 8192 {
                    ix.apply_events(std::mem::take(&mut pending))?;
                    first_event = None;
                }
            }
            if ix.has_retries() && first_event.is_none() && now.duration_since(last_event) >= Duration::from_secs(1) {
                ix.idle_tick()?;
            }
            if let Some(p) = next_periodic {
                if now >= p {
                    ix.schedule_all(Mode::Quick)?;
                    next_periodic = periodic.map(|d| now + d);
                }
            }
            if ix.has_work() && now >= scan_not_before {
                if !scanning {
                    scanning = true;
                    status.set_state(State::Scanning);
                }
                let t0 = Instant::now();
                ix.scan_slice(Duration::from_millis(100))?;
                let spent = t0.elapsed();
                // Duty cycle: rest (100-p)/p of the time spent working.
                scan_not_before = Instant::now() + spent * (100 - duty) / duty;
                if !ix.has_work() {
                    status.last_pass_end.store(now_unix(), Relaxed);
                    let _ = ix.writer().checkpoint();
                    refresh_counts(&ix, status);
                }
            }
            if !ix.has_work() && first_event.is_none() && scanning {
                scanning = false;
                status.set_state(State::Idle);
            } else if !ix.has_work() && first_event.is_none() && status.state() != State::Idle {
                status.set_state(State::Idle);
            }
            Ok(false)
        })();
        match result {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) if e.is_corrupt() => {
                log::error!("index corruption detected ({e}); reopening with recovery");
                let (old_writer, rules, roots, ino) = ix.into_parts();
                drop(old_writer);
                match open_writer(paths) {
                    Ok(w) => {
                        ix = Indexer::new(w, rules, roots, ino, status.clone());
                        let _ = ix.sync_roots(Mode::Full);
                    }
                    Err(e) => {
                        log::error!("cannot reopen index: {e}; stopping indexer");
                        status.set_state(State::Stopping);
                        return;
                    }
                }
            }
            Err(e) => {
                log::error!("indexer error: {e}");
                // Avoid a hot error loop.
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    status.set_state(State::Stopping);
    refresh_counts(&ix, status);
    if let Err(e) = ix.into_writer().close() {
        log::error!("closing index failed: {e}");
    }
}
