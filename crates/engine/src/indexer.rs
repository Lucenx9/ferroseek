// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeps the stored tree equal to the filesystem.
//!
//! Scanning and event handling reduce to one state-based primitive:
//! *re-examine `(dir_id, name)`*, which `lstat`s the current path and
//! upserts or deletes accordingly. It is idempotent, so races between the
//! scan and events converge. See `docs/ARCHITECTURE.md`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use ferroseek_index::{Change, Id, Kind, Meta, Txn, Writer};

use crate::config::{Rules, has_cachedir_tag};
use crate::inotify::{self, Inotify, RawEvent};
use crate::status::Status;
use crate::sys::Mounts;

/// How thoroughly to reconcile a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// Skip `readdir` for directories whose mtime and inode match the last
    /// reconciliation (their set of names cannot have changed); descend
    /// into known child directories.
    Quick,
    /// `readdir` and `lstat` everything.
    Full,
}

/// Suspect entries are retried at most this many batches before falling
/// back to a full reconcile of everything.
const MAX_RETRIES: u8 = 3;

pub(crate) type Result<T> = std::result::Result<T, ferroseek_index::Error>;

pub(crate) struct Indexer {
    w: Writer,
    st: State,
}

/// Everything except the writer, so that a [`Txn`] (which borrows the
/// writer) can be used alongside mutable state.
struct State {
    #[cfg(test)]
    stat_failures: HashMap<PathBuf, i32>,
    #[cfg(test)]
    listing_failure: Option<(PathBuf, usize)>,
    rules: Rules,
    roots: Vec<PathBuf>,
    ino: Option<Inotify>,
    /// wd → directory id, and its inverse.
    watches: HashMap<i32, Id>,
    by_id: HashMap<Id, i32>,
    watch_limit_hit: bool,
    mounts: Mounts,
    /// Directories pending reconciliation (DFS stack keeps it small).
    work: Vec<(Id, Mode)>,
    /// `(dir, name, tries)` whose parent path looked stale.
    retry: Vec<(Id, Vec<u8>, u8)>,
    status: Arc<Status>,
    #[cfg_attr(not(test), allow(dead_code))]
    buf: Vec<u8>,
}

impl Indexer {
    pub fn new(w: Writer, rules: Rules, roots: Vec<PathBuf>, ino: Option<Inotify>, status: Arc<Status>) -> Indexer {
        Indexer {
            w,
            st: State {
                #[cfg(test)]
                stat_failures: HashMap::new(),
                #[cfg(test)]
                listing_failure: None,
                rules,
                roots,
                ino,
                watches: HashMap::new(),
                by_id: HashMap::new(),
                watch_limit_hit: false,
                mounts: Mounts::load(),
                work: Vec::new(),
                retry: Vec::new(),
                status,
                buf: vec![0u8; 64 * 1024],
            },
        }
    }

    pub fn writer(&self) -> &Writer {
        &self.w
    }

    pub fn into_writer(self) -> Writer {
        self.w
    }

    /// Decomposes the indexer so it can be rebuilt around a reopened
    /// writer (corruption recovery). Watches are re-added by the next pass.
    pub fn into_parts(self) -> (Writer, Rules, Vec<PathBuf>, Option<Inotify>) {
        (self.w, self.st.rules, self.st.roots, self.st.ino)
    }

    pub fn has_work(&self) -> bool {
        !self.st.work.is_empty()
    }

    pub fn has_retries(&self) -> bool {
        !self.st.retry.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn watched_dirs(&self) -> usize {
        self.st.watches.len()
    }

    /// Makes the stored roots equal to the configured ones (dropping
    /// removed roots with their subtrees) and queues a reconcile pass.
    pub fn sync_roots(&mut self, mode: Mode) -> Result<()> {
        // Opaque equality key, not a persisted format to parse. Changes to
        // its representation safely cause an extra listing pass.
        // v2 also invalidates cached listings pruned by older cache-tag handling.
        self.w.apply_policy(&format!("v2:{:?}:{:?}", self.st.rules, self.st.roots))?;
        let mut tx = self.w.transaction()?;
        for (id, path) in tx.roots()? {
            if !self.st.roots.contains(&path) {
                log::info!("root {} no longer configured; dropping it", path.display());
                self.st.drop_subtree(&mut tx, id)?;
            }
        }
        tx.commit()?;
        self.schedule_all(mode)
    }

    /// Queues a reconcile pass over every root. A new pass supersedes
    /// pending work (it covers everything); `Full` wins over `Quick`.
    pub fn schedule_all(&mut self, mode: Mode) -> Result<()> {
        let pending_full = self.st.work.iter().any(|(_, m)| *m == Mode::Full);
        let mode = if pending_full { Mode::Full } else { mode };
        self.st.mounts = Mounts::load();
        self.st.work.clear();
        let mut tx = self.w.transaction()?;
        for root in self.st.roots.clone() {
            match fs::symlink_metadata(&root) {
                Ok(m) if m.is_dir() => {
                    let id = tx.ensure_root(&root, Meta::from_metadata(&m))?;
                    self.st.work.push((id, mode));
                }
                Ok(_) => log::warn!("root {} is not a directory; skipping", root.display()),
                // Possibly an unmounted disk: keep what is indexed.
                Err(e) => log::warn!("root {} is not accessible ({e}); keeping its index", root.display()),
            }
        }
        tx.commit()?;
        self.st.status.queued_dirs.store(self.st.work.len() as u64, Relaxed);
        Ok(())
    }

    /// Reconciles queued directories until `budget` elapses or the batch is
    /// large, in one transaction. Returns the number of directories done.
    pub fn scan_slice(&mut self, budget: Duration) -> Result<usize> {
        let start = Instant::now();
        let mut done = 0;
        let mut tx = self.w.transaction()?;
        while let Some((id, mode)) = self.st.work.pop() {
            self.st.reconcile_dir(&mut tx, id, mode)?;
            done += 1;
            if start.elapsed() >= budget || tx.ops() >= 200_000 {
                break;
            }
        }
        tx.commit()?;
        self.st.status.scanned_dirs.fetch_add(done as u64, Relaxed);
        self.st.status.queued_dirs.store(self.st.work.len() as u64, Relaxed);
        self.st.status.watched_dirs.store(self.st.watches.len() as u64, Relaxed);
        Ok(done)
    }

    /// Runs queued work to completion (tests, one-shot CLI indexing).
    pub fn scan_to_completion(&mut self) -> Result<()> {
        while self.has_work() {
            self.scan_slice(Duration::from_millis(500))?;
        }
        Ok(())
    }

    /// Reads whatever inotify has queued and applies it (synchronous
    /// driving for tests and the one-shot indexer).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pump_events(&mut self) -> Result<usize> {
        let Some(ino) = self.st.ino.clone() else { return Ok(0) };
        let mut evs = Vec::new();
        let mut buf = std::mem::take(&mut self.st.buf);
        let r = ino.read_available(&mut buf, &mut evs);
        self.st.buf = buf;
        r?;
        let n = evs.len();
        self.apply_events(evs)?;
        Ok(n)
    }

    /// Applies one debounced batch of events in one transaction.
    pub fn apply_events(&mut self, evs: Vec<RawEvent>) -> Result<()> {
        let st = &mut self.st;
        let mut tx = self.w.transaction()?;
        let mut dirty: Vec<(Id, Vec<u8>)> = Vec::new();
        let mut from: HashMap<u32, (Id, Vec<u8>)> = HashMap::new();
        let mut overflow = false;
        st.status.events.fetch_add(evs.len() as u64, Relaxed);
        for ev in evs {
            if ev.mask & inotify::IN_Q_OVERFLOW != 0 {
                overflow = true;
                continue;
            }
            if ev.mask & inotify::IN_IGNORED != 0 {
                st.forget_wd(ev.wd);
                continue;
            }
            let Some(&dir) = st.watches.get(&ev.wd) else { continue };
            if ev.name.is_empty() {
                // DELETE_SELF / MOVE_SELF / UNMOUNT of the watched dir: the
                // parent's own event handles the entry.
                continue;
            }
            if ev.mask & inotify::IN_MOVED_FROM != 0 {
                if let Some(prev) = from.insert(ev.cookie, (dir, ev.name)) {
                    dirty.push(prev);
                }
            } else if ev.mask & inotify::IN_MOVED_TO != 0 {
                match from.remove(&ev.cookie) {
                    Some((fdir, fname)) => st.rename(&mut tx, fdir, fname, dir, ev.name, &mut dirty)?,
                    None => dirty.push((dir, ev.name)),
                }
            } else {
                dirty.push((dir, ev.name));
            }
        }
        // Moved out of every watched directory: resolves to a delete.
        dirty.extend(from.into_values());

        let mut seen = HashSet::new();
        for (dir, name) in dirty {
            if seen.insert((dir, name.clone())) {
                st.resolve(&mut tx, dir, &name, 0)?;
            }
        }
        // Suspects from earlier batches: by now the rename that made their
        // parent path stale has normally been applied.
        let mut fallback = false;
        for (dir, name, tries) in std::mem::take(&mut st.retry_previous()) {
            if tries >= MAX_RETRIES {
                fallback = true;
            } else {
                st.resolve(&mut tx, dir, &name, tries + 1)?;
            }
        }
        tx.commit()?;
        st.status.watched_dirs.store(st.watches.len() as u64, Relaxed);
        if overflow {
            st.status.overflows.fetch_add(1, Relaxed);
            log::warn!("inotify queue overflowed; reconciling all roots");
        }
        if overflow || fallback {
            self.schedule_all(Mode::Full)?;
        }
        Ok(())
    }

    /// Called when the event stream has been quiet: unresolved suspects
    /// cannot be explained by pending events any more.
    pub fn idle_tick(&mut self) -> Result<()> {
        if self.st.retry.is_empty() {
            return Ok(());
        }
        self.st.retry.clear();
        self.schedule_all(Mode::Full)
    }

    /// Drops every indexed entry and watch, then queues a full scan.
    pub fn rebuild(&mut self) -> Result<()> {
        let mut tx = self.w.transaction()?;
        for (id, _) in tx.roots()? {
            self.st.drop_subtree(&mut tx, id)?;
        }
        tx.commit()?;
        self.st.retry.clear();
        self.sync_roots(Mode::Full)
    }
}

impl State {
    fn metadata(&self, path: &Path) -> io::Result<fs::Metadata> {
        #[cfg(test)]
        if let Some(errno) = self.stat_failures.get(path) {
            return Err(io::Error::from_raw_os_error(*errno));
        }
        fs::symlink_metadata(path)
    }

    /// Suspects carried over from the previous batch. New suspects found
    /// while resolving the current batch go to `self.retry` for the next.
    fn retry_previous(&mut self) -> Vec<(Id, Vec<u8>, u8)> {
        std::mem::take(&mut self.retry)
    }

    fn watch(&mut self, path: &Path, id: Id) {
        let Some(ino) = &self.ino else { return };
        match ino.add_watch(path, inotify::DIR_MASK) {
            Ok(wd) => {
                if let Some(prev) = self.watches.insert(wd, id)
                    && prev != id
                    && self.by_id.get(&prev) == Some(&wd)
                {
                    self.by_id.remove(&prev);
                }
                if let Some(old) = self.by_id.insert(id, wd)
                    && old != wd
                    && self.watches.get(&old) == Some(&id)
                {
                    self.watches.remove(&old);
                }
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOSPC) => {
                if !self.watch_limit_hit {
                    log::warn!(
                        "inotify watch limit reached (fs.inotify.max_user_watches); \
                         further directories are only covered by periodic reconciliation"
                    );
                }
                self.watch_limit_hit = true;
                self.status.watch_limit_hit.store(true, Relaxed);
            }
            Err(e) => log::debug!("cannot watch {}: {e}", path.display()),
        }
    }

    fn unwatch(&mut self, id: Id) {
        if let Some(wd) = self.by_id.remove(&id) {
            self.watches.remove(&wd);
            if let Some(ino) = &self.ino {
                // EINVAL if the kernel already dropped it (directory deleted).
                let _ = ino.rm_watch(wd);
            }
        }
    }

    fn forget_wd(&mut self, wd: i32) {
        if let Some(id) = self.watches.remove(&wd)
            && self.by_id.get(&id) == Some(&wd)
        {
            self.by_id.remove(&id);
        }
    }

    fn drop_subtree(&mut self, tx: &mut Txn<'_>, id: Id) -> Result<()> {
        for d in tx.subtree_dirs(id)? {
            self.unwatch(d);
        }
        tx.delete(id)?;
        Ok(())
    }

    fn drop_children(&mut self, tx: &mut Txn<'_>, id: Id) -> Result<()> {
        for (_, c) in tx.children(id)? {
            self.drop_subtree(tx, c.id)?;
        }
        Ok(())
    }

    fn depth(&self, path: &Path) -> u32 {
        let root = self.roots.iter().filter(|r| path.starts_with(r)).map(|r| r.components().count()).max();
        root.map(|r| path.components().count().saturating_sub(r) as u32).unwrap_or(0)
    }

    fn is_foreign_mount(&self, parent_dev: u64, md: &fs::Metadata, path: &Path) -> bool {
        self.rules.stay_on_filesystem
            && md.dev() != parent_dev
            && self.mounts.is_mount_point(path)
            && !self.roots.iter().any(|r| r == path)
    }

    fn reconcile_dir(&mut self, tx: &mut Txn<'_>, id: Id, mode: Mode) -> Result<()> {
        let Some(path) = tx.path(id)? else { return Ok(()) };
        let Some(stored) = tx.meta(id)? else { return Ok(()) };
        if stored.kind != Kind::Dir {
            return Ok(());
        }
        let depth = self.depth(&path);
        if depth > self.rules.max_depth {
            self.drop_children(tx, id)?;
            self.unwatch(id);
            return Ok(());
        }
        // Watch before listing, so nothing created after the listing is
        // missed.
        self.watch(&path, id);
        let md = match self.metadata(&path) {
            Ok(m) if m.is_dir() => m,
            // Gone or replaced by a non-directory: removal is the parent's
            // (or an event's) job, keyed by name.
            Ok(_) => return Ok(()),
            Err(e) => {
                log::debug!("cannot stat {}: {e}", path.display());
                self.status.errors.fetch_add(1, Relaxed);
                tx.mark_dir_reconciled(id, 0, stored.ino)?;
                return Ok(());
            }
        };
        let cur = Meta::from_metadata(&md);
        if self.rules.respect_cachedir_tag && has_cachedir_tag(&path) {
            self.drop_children(tx, id)?;
            // Tag contents can change without changing the directory mtime.
            tx.mark_dir_reconciled(id, 0, cur.ino)?;
            return Ok(());
        }
        if mode == Mode::Quick && stored.mtime_ns != 0 && stored.mtime_ns == cur.mtime_ns && stored.ino == cur.ino {
            if depth < self.rules.max_depth {
                for (cid, _) in tx.child_dirs(id)? {
                    self.work.push((cid, mode));
                }
            }
            return Ok(());
        }
        let rd = match fs::read_dir(&path) {
            Ok(rd) => rd,
            Err(e) => {
                log::debug!("cannot list {}: {e}", path.display());
                self.status.errors.fetch_add(1, Relaxed);
                tx.mark_dir_reconciled(id, 0, cur.ino)?;
                return Ok(());
            }
        };
        #[cfg(test)]
        let rd = rd
            .enumerate()
            .map(|(i, entry)| {
                if self.listing_failure.as_ref() == Some(&(path.clone(), i)) {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    entry
                }
            })
            .collect::<Vec<_>>()
            .into_iter();
        let mut stored_children: HashMap<Vec<u8>, ferroseek_index::Child> = tx.children(id)?.into_iter().collect();
        let mut complete = true;
        for ent in rd {
            let ent = match ent {
                Ok(ent) => ent,
                Err(e) => {
                    log::debug!("incomplete listing of {}: {e}", path.display());
                    self.status.errors.fetch_add(1, Relaxed);
                    complete = false;
                    continue;
                }
            };
            let name = ent.file_name().as_bytes().to_vec();
            if self.rules.excluded(&path, &name) {
                continue;
            }
            let full = path.join(OsStr::from_bytes(&name));
            let cmd = match self.metadata(&full) {
                Ok(m) => m,
                Err(e) => {
                    // Seeing the name does not prove that an inaccessible entry is gone.
                    stored_children.remove(&name);
                    log::debug!("cannot stat {}: {e}", full.display());
                    self.status.errors.fetch_add(1, Relaxed);
                    complete = false;
                    continue;
                }
            };
            let meta = Meta::from_metadata(&cmd);
            let old = stored_children.remove(&name);
            let unchanged = match &old {
                Some(o) if o.meta.kind == Kind::Dir => meta.kind == Kind::Dir && o.meta.ino == meta.ino,
                Some(o) => o.meta == meta,
                None => false,
            };
            let cid = match (&old, unchanged) {
                (Some(o), true) => o.id,
                (Some(o), false) if o.meta.kind == Kind::Dir && meta.kind != Kind::Dir => {
                    self.drop_children(tx, o.id)?;
                    tx.upsert(id, &name, meta)?.0
                }
                _ => tx.upsert(id, &name, meta)?.0,
            };
            if meta.kind == Kind::Dir {
                if self.is_foreign_mount(md.dev(), &cmd, &full) {
                    self.drop_children(tx, cid)?;
                } else if depth < self.rules.max_depth {
                    self.work.push((cid, mode));
                } else {
                    self.drop_children(tx, cid)?;
                    self.unwatch(cid);
                }
            }
        }
        // Absence is authoritative only after a complete listing. Invalidating
        // the timestamp makes the next quick pass retry without a busy loop.
        if complete {
            for (_, c) in stored_children {
                self.drop_subtree(tx, c.id)?;
            }
        }
        tx.mark_dir_reconciled(id, if complete { cur.mtime_ns } else { 0 }, cur.ino)?;
        Ok(())
    }

    /// The state-based primitive: make `(dir, name)` match the filesystem.
    fn resolve(&mut self, tx: &mut Txn<'_>, dir: Id, name: &[u8], tries: u8) -> Result<()> {
        let Some(dpath) = tx.path(dir)? else { return Ok(()) };
        if self.rules.respect_cachedir_tag {
            if has_cachedir_tag(&dpath) {
                self.drop_children(tx, dir)?;
                if let Some(meta) = tx.meta(dir)? {
                    tx.mark_dir_reconciled(dir, 0, meta.ino)?;
                }
                return Ok(());
            }
            if name == b"CACHEDIR.TAG" {
                self.work.push((dir, Mode::Full));
            }
        }
        let existing = tx.child(dir, name)?;
        if self.rules.excluded(&dpath, name) {
            if let Some(c) = existing {
                self.drop_subtree(tx, c.id)?;
            }
            return Ok(());
        }
        let full = dpath.join(OsStr::from_bytes(name));
        match self.metadata(&full) {
            Ok(md) => {
                let meta = Meta::from_metadata(&md);
                if let Some(c) = &existing
                    && c.meta.kind == Kind::Dir
                    && meta.kind != Kind::Dir
                {
                    self.drop_children(tx, c.id)?;
                }
                let (id, change) = tx.upsert(dir, name, meta)?;
                if meta.kind == Kind::Dir {
                    let parent_dev = fs::symlink_metadata(&dpath).map(|m| m.dev()).unwrap_or(md.dev());
                    if self.is_foreign_mount(parent_dev, &md, &full) {
                        // Not descending into another filesystem.
                    } else if (change != Change::Unchanged || !self.by_id.contains_key(&id))
                        && self.depth(&full) <= self.rules.max_depth
                    {
                        self.work.push((id, Mode::Quick));
                    }
                } else if name == b"CACHEDIR.TAG" && self.rules.respect_cachedir_tag {
                    self.work.push((dir, Mode::Quick));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => {
                // Missing. Only trust that if the parent is still the
                // directory we think it is; otherwise our path for it is
                // stale (a rename not yet applied) and deleting would be
                // wrong.
                let parent_ok = match (fs::symlink_metadata(&dpath), tx.meta(dir)?) {
                    (Ok(pm), Some(stored)) => pm.is_dir() && (stored.ino == 0 || pm.ino() == stored.ino),
                    _ => false,
                };
                if parent_ok {
                    if let Some(c) = existing {
                        self.drop_subtree(tx, c.id)?;
                    }
                } else {
                    self.retry.push((dir, name.to_vec(), tries));
                }
            }
            Err(e) => {
                log::debug!("cannot stat {}: {e}", full.display());
                self.status.errors.fetch_add(1, Relaxed);
                self.retry.push((dir, name.to_vec(), tries));
            }
        }
        Ok(())
    }

    fn rename(
        &mut self,
        tx: &mut Txn<'_>,
        fdir: Id,
        fname: Vec<u8>,
        tdir: Id,
        tname: Vec<u8>,
        dirty: &mut Vec<(Id, Vec<u8>)>,
    ) -> Result<()> {
        let src = tx.child(fdir, &fname)?;
        let Some(tdpath) = tx.path(tdir)? else {
            dirty.push((fdir, fname));
            return Ok(());
        };
        match src {
            Some(c) if !self.rules.excluded(&tdpath, &tname) => {
                if let Some(dst) = tx.child(tdir, &tname)?
                    && dst.id != c.id
                {
                    self.drop_subtree(tx, dst.id)?;
                }
                if c.meta.kind == Kind::Dir
                    && let Some(old_path) = tx.path(c.id)?
                {
                    let new_path = tdpath.join(OsStr::from_bytes(&tname));
                    if self.depth(&old_path) != self.depth(&new_path)
                        || self.rules.has_exclusions_below(&old_path)
                        || self.rules.has_exclusions_below(&new_path)
                    {
                        // A move changes path policy without changing listing mtimes.
                        // Persist invalidation in the same transaction as the rename.
                        tx.invalidate_subtree_listings(c.id)?;
                        self.work.push((c.id, Mode::Quick));
                    }
                }
                tx.rename(c.id, tdir, &tname)?;
                // Refresh metadata (cheap) and pick up anything unusual.
                dirty.push((tdir, tname));
            }
            Some(c) => self.drop_subtree(tx, c.id)?,
            None => {
                dirty.push((fdir, fname));
                dirty.push((tdir, tname));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "indexer_tests.rs"]
mod tests;
