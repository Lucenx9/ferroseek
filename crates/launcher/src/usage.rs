// SPDX-License-Identifier: GPL-3.0-or-later
//! Launch history: which results the user actually opens, and what they
//! had typed when they did.
//!
//! Stored by the launcher alone in `$XDG_STATE_HOME/ferroseek/launcher-usage`
//! (never in the file index). Each record keeps an exponentially decayed use
//! count, so habits fade after a few weeks. Two signals feed the ranking
//! boost: how often an item is opened at all, and how often it is opened
//! from a related query (one is a prefix of the other: choosing Firefox
//! after typing "fir" also favours it for "fi" and "fire"). The boost is
//! capped, and the model only lets it reorder matches of the same class.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 3600.0;
/// Items remembered (by overall use) and queries remembered per item.
const MAX_ITEMS: usize = 400;
const MAX_QUERIES_PER_ITEM: usize = 8;
/// Stored queries are normalized and cut to this many characters.
const MAX_QUERY_CHARS: usize = 32;
/// Boost from overall use and from related queries, and their sum's cap
/// (see `matcher`: whole-name 1000, prefix ≤ 900, word 800, substring 600).
const MAX_GENERAL_BOOST: f64 = 60.0;
const MAX_QUERY_BOOST: f64 = 90.0;
pub const MAX_BOOST: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    App,
    File,
}

impl Kind {
    fn tags(self) -> (&'static str, &'static str) {
        match self {
            Kind::App => ("app", "qapp"),
            Kind::File => ("file", "qfile"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    weight: f64,
    last: u64,
}

impl Entry {
    fn decayed(&self, now: u64) -> f64 {
        let dt = now.saturating_sub(self.last) as f64;
        self.weight * 0.5f64.powf(dt / HALF_LIFE_SECS)
    }

    fn bump(&mut self, now: u64) {
        self.weight = self.decayed(now) + 1.0;
        self.last = now;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Record {
    /// Every open of the item.
    general: Option<Entry>,
    /// Opens per normalized query.
    by_query: Vec<(String, Entry)>,
}

type Records = HashMap<(Kind, String), Record>;

#[derive(Debug, Default)]
pub struct Usage {
    path: Option<PathBuf>,
    entries: Records,
}

/// Lower-case, single-spaced, at most `MAX_QUERY_CHARS`; `None` if empty.
pub fn normalize_query(q: &str) -> Option<String> {
    let n: String =
        q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().chars().take(MAX_QUERY_CHARS).collect();
    (!n.is_empty()).then_some(n)
}

fn related(stored: &str, current: &str) -> bool {
    stored.starts_with(current) || current.starts_with(stored)
}

impl Usage {
    /// Default location under `$XDG_STATE_HOME`.
    pub fn default_path() -> Option<PathBuf> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
        Some(state.join("ferroseek/launcher-usage"))
    }

    /// Loads the history; a missing or unreadable file is an empty history.
    pub fn load(path: Option<PathBuf>) -> Usage {
        let entries = path.as_deref().map(read_entries).unwrap_or_default();
        Usage { path, entries }
    }

    /// A history that records nothing to disk (`--private`).
    pub fn disabled() -> Usage {
        Usage::default()
    }

    /// Ranking boost in `0..=MAX_BOOST` for `key` while `query` is typed.
    pub fn boost(&self, kind: Kind, key: &str, query: &str, now: u64) -> u32 {
        let Some(r) = self.entries.get(&(kind, key.to_string())) else { return 0 };
        // log2 grows slowly: 1 use ≈ 20 (40 for a related query), 3 ≈ 40 (80).
        let general = r.general.map_or(0.0, |e| ((1.0 + e.decayed(now)).log2() * 20.0).min(MAX_GENERAL_BOOST));
        let specific = normalize_query(query).map_or(0.0, |q| {
            let w: f64 = r.by_query.iter().filter(|(s, _)| related(s, &q)).map(|(_, e)| e.decayed(now)).sum();
            ((1.0 + w).log2() * 40.0).min(MAX_QUERY_BOOST)
        });
        (general + specific).min(MAX_BOOST as f64) as u32
    }

    /// Most used items, best first, for the empty query.
    pub fn recent(&self, now: u64, limit: usize) -> Vec<(Kind, String)> {
        let mut v: Vec<_> =
            self.entries.iter().filter_map(|(k, r)| Some((r.general?.decayed(now), k.clone()))).collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        v.into_iter().filter(|(w, _)| *w >= 0.05).take(limit).map(|(_, k)| k).collect()
    }

    /// Records one open of `key` from `query`. With a backing file, the
    /// update is a locked read-modify-write of the file, so several
    /// launchers (or a concurrent `--forget-history`) never overwrite each
    /// other's changes. Keys are single-line (desktop IDs and
    /// percent-encoded file URLs); others are ignored.
    pub fn record(&mut self, kind: Kind, key: &str, query: &str, now: u64) {
        if key.is_empty() || key.contains(['\n', '\r', '\t']) {
            return;
        }
        let query = normalize_query(query);
        let Some(path) = self.path.clone() else {
            add_use(&mut self.entries, kind, key, query.as_deref(), now);
            return;
        };
        let r = with_lock(&path, true, || {
            // The file is authoritative: another instance may have added
            // uses, or the history may have been forgotten, since load.
            self.entries = read_entries(&path);
            add_use(&mut self.entries, kind, key, query.as_deref(), now);
            write_entries(&path, &self.entries)
        });
        if let Err(e) = r {
            log::warn!("cannot save launcher history {}: {e}", path.display());
        }
    }

    /// Removes all ranking signals for one item, preserving other instances'
    /// updates. Returns `WouldBlock` when another writer holds the lock;
    /// a failed reset leaves the in-memory ranking unchanged.
    pub fn reset(&mut self, kind: Kind, key: &str) -> std::io::Result<()> {
        let item = (kind, key.to_string());
        let Some(path) = self.path.clone() else {
            self.entries.remove(&item);
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            match std::fs::metadata(parent) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.entries.clear();
                    return Ok(());
                }
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        let entries = with_lock(&path, false, || {
            let mut entries = try_read_entries(&path)?;
            if entries.remove(&item).is_some() {
                write_entries(&path, &entries)?;
            }
            Ok(entries)
        })?;
        self.entries = entries;
        Ok(())
    }

    /// Deletes the stored history (`--forget-history`), under the same
    /// lock as updates.
    pub fn forget(path: &Path) -> std::io::Result<()> {
        if !path.parent().is_some_and(Path::exists) {
            return Ok(());
        }
        with_lock(path, true, || match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        })
    }
}

fn add_use(entries: &mut Records, kind: Kind, key: &str, query: Option<&str>, now: u64) {
    let r = entries.entry((kind, key.to_string())).or_default();
    r.general.get_or_insert(Entry { weight: 0.0, last: now }).bump(now);
    if let Some(q) = query {
        match r.by_query.iter_mut().find(|(s, _)| s == q) {
            Some((_, e)) => e.bump(now),
            None => r.by_query.push((q.to_string(), Entry { weight: 1.0, last: now })),
        }
        if r.by_query.len() > MAX_QUERIES_PER_ITEM {
            r.by_query.sort_by(|a, b| b.1.decayed(now).total_cmp(&a.1.decayed(now)));
            r.by_query.truncate(MAX_QUERIES_PER_ITEM);
        }
    }
    if entries.len() > MAX_ITEMS {
        let weight = |r: &Record| r.general.map_or(0.0, |e| e.decayed(now));
        let mut by_weight: Vec<_> = entries.iter().map(|(k, r)| (weight(r), k.clone())).collect();
        by_weight.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, k) in by_weight.into_iter().take(entries.len() - MAX_ITEMS) {
            entries.remove(&k);
        }
    }
}

/// Lines: `app|file <weight> <last> <key>` for overall use and
/// `qapp|qfile <weight> <last> <query> <key>` per query, tab-separated.
/// Unknown or malformed lines are skipped.
fn read_entries(path: &Path) -> Records {
    try_read_entries(path).unwrap_or_default()
}

fn try_read_entries(path: &Path) -> std::io::Result<Records> {
    let mut entries = Records::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(e) => return Err(e),
    };
    for line in text.lines() {
        let mut f = line.splitn(2, '\t');
        let (Some(tag), Some(rest)) = (f.next(), f.next()) else { continue };
        let (kind, with_query) = match tag {
            "app" => (Kind::App, false),
            "file" => (Kind::File, false),
            "qapp" => (Kind::App, true),
            "qfile" => (Kind::File, true),
            _ => continue,
        };
        let mut f = rest.splitn(if with_query { 4 } else { 3 }, '\t');
        let (Some(weight), Some(last)) = (f.next(), f.next()) else { continue };
        let query = if with_query { f.next().and_then(normalize_query) } else { None };
        let Some(key) = f.next().filter(|k| !k.is_empty()) else { continue };
        let (Ok(weight), Ok(last)) = (weight.parse::<f64>(), last.parse::<u64>()) else { continue };
        if !weight.is_finite() || weight <= 0.0 || (with_query && query.is_none()) {
            continue;
        }
        let e = Entry { weight, last };
        let r = entries.entry((kind, key.to_string())).or_default();
        match query {
            Some(q) if r.by_query.len() < MAX_QUERIES_PER_ITEM && !r.by_query.iter().any(|(s, _)| *s == q) => {
                r.by_query.push((q, e))
            }
            Some(_) => {}
            None => r.general = Some(e),
        }
    }
    Ok(entries)
}

/// Owner-only file creation: the history names files the user opened.
fn private_options() -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = std::fs::OpenOptions::new();
    o.mode(0o600);
    o
}

/// Runs `f` holding an exclusive lock on `<path>.lock`, creating the
/// directory owner-only if needed. Without `wait`, contention returns an error.
fn with_lock<T>(path: &Path, wait: bool, f: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    use std::os::unix::fs::DirBuilderExt;
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let lock =
        private_options().read(true).write(true).create(true).truncate(false).open(path.with_extension("lock"))?;
    if wait {
        lock.lock()?;
    } else {
        lock.try_lock()?;
    }
    let r = f();
    let _ = lock.unlock();
    r
}

/// Atomic replace: a new owner-only sibling file, then rename.
fn write_entries(path: &Path, entries: &Records) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp); // left by a crashed run with this PID
    {
        let mut f = private_options().write(true).create_new(true).open(&tmp)?;
        let mut rows: Vec<_> = entries.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        for ((kind, key), r) in rows {
            let (tag, qtag) = kind.tags();
            if let Some(e) = r.general {
                writeln!(f, "{tag}\t{}\t{}\t{key}", e.weight, e.last)?;
            }
            for (q, e) in &r.by_query {
                writeln!(f, "{qtag}\t{}\t{}\t{q}\t{key}", e.weight, e.last)?;
            }
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 3600;

    #[test]
    fn boost_grows_slowly_is_capped_and_decays() {
        let mut u = Usage::disabled();
        let t = 1_000_000;
        assert_eq!(u.boost(Kind::App, "a.desktop", "", t), 0);
        u.record(Kind::App, "a.desktop", "", t);
        let one = u.boost(Kind::App, "a.desktop", "", t);
        for _ in 0..100 {
            u.record(Kind::App, "a.desktop", "", t);
        }
        let many = u.boost(Kind::App, "a.desktop", "", t);
        assert!(one > 0 && one < many && many == MAX_GENERAL_BOOST as u32, "{one} {many}");
        assert!(u.boost(Kind::App, "a.desktop", "", t + 60 * DAY) < many);
        assert_eq!(u.boost(Kind::File, "a.desktop", "", t), 0, "kinds are separate");
    }

    #[test]
    fn persists_round_trip_and_ignores_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state/ferroseek/launcher-usage");
        let mut u = Usage::load(Some(p.clone()));
        u.record(Kind::App, "org.kde.konsole.desktop", "", 100);
        u.record(Kind::File, "file:///t/a%20b", "", 100);
        u.record(Kind::File, "bad\nkey", "", 100);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 2);
        std::fs::write(&p, format!("{text}junk\nfile\tNaN\t1\tx\nzzz\t1\t1\tx\napp\t1\t1\t\n")).unwrap();
        let v = Usage::load(Some(p.clone()));
        assert_eq!(v.entries.len(), 2);
        assert_eq!(
            v.boost(Kind::App, "org.kde.konsole.desktop", "", 100),
            u.boost(Kind::App, "org.kde.konsole.desktop", "", 100)
        );
        Usage::forget(&p).unwrap();
        Usage::forget(&p).unwrap();
        assert!(Usage::load(Some(p)).entries.is_empty());
    }

    #[test]
    fn updates_merge_with_the_file_and_respect_forget() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state/ferroseek/launcher-usage");
        let mut a = Usage::load(Some(p.clone()));
        let mut b = Usage::load(Some(p.clone()));
        a.record(Kind::App, "a.desktop", "", 100);
        b.record(Kind::App, "b.desktop", "", 100);
        let disk = Usage::load(Some(p.clone()));
        assert!(
            disk.boost(Kind::App, "a.desktop", "", 100) > 0 && disk.boost(Kind::App, "b.desktop", "", 100) > 0,
            "no lost update"
        );

        Usage::forget(&p).unwrap();
        a.record(Kind::App, "c.desktop", "", 100);
        let disk = Usage::load(Some(p.clone()));
        assert_eq!(disk.boost(Kind::App, "a.desktop", "", 100), 0, "forgotten history must not come back");
        assert_eq!(disk.boost(Kind::App, "b.desktop", "", 100), 0);
        assert!(disk.boost(Kind::App, "c.desktop", "", 100) > 0);

        let mode = |q: &Path| std::fs::metadata(q).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(&p.with_extension("lock")), 0o600);
        assert_eq!(mode(p.parent().unwrap()), 0o700);
        Usage::forget(&dir.path().join("missing/dir/launcher-usage")).unwrap();
        assert!(!dir.path().join("missing").exists(), "forget creates nothing");
    }

    #[test]
    fn recent_orders_by_decayed_weight_and_caps_size() {
        let mut u = Usage::disabled();
        u.record(Kind::App, "old", "", 0);
        u.record(Kind::App, "old", "", 0);
        u.record(Kind::App, "new", "", 40 * DAY);
        assert_eq!(u.recent(40 * DAY, 10), [(Kind::App, "new".to_string()), (Kind::App, "old".to_string())]);
        for i in 0..(MAX_ITEMS + 50) {
            u.record(Kind::File, &format!("f{i}"), "", 50 * DAY);
        }
        assert_eq!(u.entries.len(), MAX_ITEMS);
    }

    #[test]
    fn related_queries_add_a_specific_boost() {
        let mut u = Usage::disabled();
        let t = 1_000;
        u.record(Kind::App, "firefox.desktop", "Fir", t);
        let general_only = u.boost(Kind::App, "firefox.desktop", "", t);
        for q in ["fi", "fir", "FIRE  fox", "f"] {
            assert!(u.boost(Kind::App, "firefox.desktop", q, t) > general_only, "{q}");
        }
        assert_eq!(u.boost(Kind::App, "firefox.desktop", "files", t), general_only, "unrelated query");
        for _ in 0..50 {
            u.record(Kind::App, "firefox.desktop", "fir", t);
        }
        assert_eq!(u.boost(Kind::App, "firefox.desktop", "fi", t), MAX_BOOST);
        for i in 0..20 {
            u.record(Kind::App, "firefox.desktop", &format!("q{i}"), t);
        }
        let r = &u.entries[&(Kind::App, "firefox.desktop".to_string())];
        assert_eq!(r.by_query.len(), MAX_QUERIES_PER_ITEM);
        assert!(r.by_query.iter().any(|(q, _)| q == "fir"), "the strongest query is kept");
        assert_eq!(normalize_query(&"x".repeat(100)).unwrap().len(), MAX_QUERY_CHARS);
        assert_eq!(normalize_query("  \t "), None);
    }

    #[test]
    fn query_records_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("launcher-usage");
        let mut u = Usage::load(Some(p.clone()));
        u.record(Kind::File, "file:///t/a", "rep ort", 5);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "file\t1\t5\tfile:///t/a\nqfile\t1\t5\trep ort\tfile:///t/a\n");
        let v = Usage::load(Some(p));
        assert_eq!(v.boost(Kind::File, "file:///t/a", "rep", 5), u.boost(Kind::File, "file:///t/a", "rep", 5));
        assert!(v.boost(Kind::File, "file:///t/a", "rep", 5) > v.boost(Kind::File, "file:///t/a", "", 5));
    }

    #[test]
    fn reset_from_a_stale_instance_preserves_other_updates_and_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("launcher-usage");
        let mut writer = Usage::load(Some(p.clone()));
        writer.record(Kind::App, "shared", "fir", 100);
        let mut stale = Usage::load(Some(p.clone()));
        writer.record(Kind::App, "shared", "fire", 100);
        writer.record(Kind::File, "shared", "fir", 100);
        writer.record(Kind::App, "other", "oth", 100);
        let mut other_records = writer.entries.clone();
        other_records.remove(&(Kind::App, "shared".into()));

        stale.reset(Kind::App, "shared").unwrap();
        assert_eq!(stale.entries, other_records);
        assert_eq!(Usage::load(Some(p.clone())).entries, other_records);
        assert!(stale.boost(Kind::File, "shared", "fir", 100) > 0);

        // A writer loaded before reset must not bring the removed record back.
        writer.record(Kind::App, "new", "n", 100);
        let disk = Usage::load(Some(p));
        assert_eq!(disk.boost(Kind::App, "shared", "fir", 100), 0);
        assert!(disk.boost(Kind::App, "new", "n", 100) > 0);
        assert!(disk.boost(Kind::App, "other", "oth", 100) > 0);
    }

    #[test]
    fn reset_removes_overall_and_related_query_boosts_and_new_use_starts_fresh() {
        let mut u = Usage::disabled();
        for _ in 0..5 {
            u.record(Kind::App, "firefox.desktop", "fir", 100);
        }
        u.record(Kind::App, "firefox.desktop", "browser", 100);
        u.reset(Kind::App, "firefox.desktop").unwrap();
        for q in ["", "f", "fir", "firefox", "browser"] {
            assert_eq!(u.boost(Kind::App, "firefox.desktop", q, 100), 0, "{q:?}");
        }
        assert!(u.recent(100, 8).is_empty());
        u.record(Kind::App, "firefox.desktop", "fir", 100);
        let mut fresh = Usage::disabled();
        fresh.record(Kind::App, "firefox.desktop", "fir", 100);
        assert_eq!(u.entries, fresh.entries);
    }

    #[test]
    fn reset_missing_item_does_not_create_or_rewrite_history() {
        let dir = tempfile::tempdir().unwrap();
        let missing_parent = dir.path().join("missing/launcher-usage");
        Usage::load(Some(missing_parent.clone())).reset(Kind::App, "absent").unwrap();
        assert!(!missing_parent.parent().unwrap().exists());

        let p = dir.path().join("launcher-usage");
        let mut u = Usage::load(Some(p.clone()));
        u.reset(Kind::App, "absent").unwrap();
        assert!(!p.exists());
        u.record(Kind::App, "present", "p", 100);
        let before = std::fs::read(&p).unwrap();
        u.reset(Kind::App, "absent").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), before);
        Usage::forget(&p).unwrap();
        u.reset(Kind::App, "present").unwrap();
        assert!(u.entries.is_empty());
        assert!(!p.exists(), "reset must not resurrect forgotten history");
    }

    #[test]
    fn reset_propagates_read_and_write_errors_without_changing_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("launcher-usage");
        let mut u = Usage::load(Some(p.clone()));
        u.record(Kind::App, "app", "a", 100);
        let before = u.entries.clone();
        let disk = std::fs::read(&p).unwrap();
        let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::create_dir(&tmp).unwrap();
        assert!(u.reset(Kind::App, "app").is_err());
        assert_eq!(u.entries, before);
        assert_eq!(std::fs::read(&p).unwrap(), disk);

        std::fs::remove_dir(&tmp).unwrap();
        std::fs::remove_file(&p).unwrap();
        std::fs::create_dir(&p).unwrap();
        assert!(u.reset(Kind::App, "app").is_err());
        assert_eq!(u.entries, before);
    }

    #[test]
    fn reset_returns_without_waiting_for_a_busy_history_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("launcher-usage");
        let mut u = Usage::load(Some(p.clone()));
        u.record(Kind::App, "app", "a", 100);
        let before = u.entries.clone();
        let disk = std::fs::read(&p).unwrap();
        let lock = private_options().read(true).write(true).open(p.with_extension("lock")).unwrap();
        lock.lock().unwrap();

        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = u.reset(Kind::App, "app");
            tx.send((u, result)).unwrap();
        });
        let received = rx.recv_timeout(Duration::from_secs(2));
        // Release before asserting so a blocking-lock regression cannot hang.
        lock.unlock().unwrap();
        worker.join().unwrap();
        let (mut u, result) = received.expect("reset must return while the history lock is held");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(u.entries, before);
        assert_eq!(std::fs::read(&p).unwrap(), disk);

        u.reset(Kind::App, "app").unwrap();
        assert!(u.entries.is_empty());
        assert!(Usage::load(Some(p)).entries.is_empty());
    }

    #[test]
    fn reset_without_a_backing_path_only_changes_session_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("launcher-usage");
        let mut persistent = Usage::load(Some(p.clone()));
        persistent.record(Kind::App, "app", "a", 100);
        let before = std::fs::read(&p).unwrap();
        let mut private = Usage::disabled();
        private.record(Kind::App, "app", "a", 100);
        private.record(Kind::File, "app", "a", 100);
        private.reset(Kind::App, "app").unwrap();
        assert_eq!(private.boost(Kind::App, "app", "a", 100), 0);
        assert!(private.boost(Kind::File, "app", "a", 100) > 0);
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }
}
