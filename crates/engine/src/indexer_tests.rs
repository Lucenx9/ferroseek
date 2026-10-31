// SPDX-License-Identifier: GPL-3.0-or-later
//! Behavioural tests for the indexer against real directories and real
//! inotify, driven synchronously. Everything lives in temp dirs.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferroseek_index::{Query, Reader, SearchOptions, Writer};

use super::{Indexer, Mode};
use crate::config::{Config, Rules};
use crate::inotify::{IN_Q_OVERFLOW, Inotify, RawEvent};
use crate::status::Status;

struct Fx {
    _t: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
    ix: Indexer,
}

fn fx_with(cfg: Config) -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("root");
    fs::create_dir(&root).unwrap();
    let db = t.path().join("index.sqlite");
    let ix = make_ix(&db, &root, &cfg, true);
    Fx { _t: t, root, db, ix }
}

fn make_ix(db: &Path, root: &Path, cfg: &Config, watch: bool) -> Indexer {
    let (w, _) = Writer::open(db).unwrap();
    let ino = watch.then(|| Inotify::new().unwrap());
    Indexer::new(w, Rules::new(cfg, &[]), vec![root.to_path_buf()], ino, Arc::new(Status::default()))
}

fn fx() -> Fx {
    fx_with(Config::default())
}

impl Fx {
    fn scan(&mut self, mode: Mode) {
        self.ix.sync_roots(mode).unwrap();
        self.ix.scan_to_completion().unwrap();
    }

    /// Applies all queued events and any follow-up scan work, until quiet.
    fn settle(&mut self) {
        for _ in 0..20 {
            let n = self.ix.pump_events().unwrap();
            self.ix.scan_to_completion().unwrap();
            if n == 0 && !self.ix.has_work() {
                return;
            }
        }
        panic!("did not settle");
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Paths (relative to root) of all hits for `q`.
    fn find(&self, q: &str) -> Vec<String> {
        let r = Reader::open(&self.db).unwrap();
        let opts = SearchOptions { limit: 1000, ..Default::default() };
        let mut v: Vec<String> = r
            .search(&Query::parse(q), &opts)
            .unwrap()
            .into_iter()
            .map(|h| h.path.strip_prefix(&self.root).unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn indexed(&self, rel: &str) -> bool {
        Reader::open(&self.db).unwrap().lookup(&self.p(rel)).unwrap().is_some()
    }

    fn count(&self) -> u64 {
        let s = Reader::open(&self.db).unwrap().stats().unwrap();
        s.files + s.dirs + s.other
    }
}

fn touch(p: &Path) {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(p, b"x").unwrap();
}

#[test]
fn initial_scan_respects_exclusions() {
    let mut f = fx();
    touch(&f.p("docs/report.txt"));
    touch(&f.p("docs/.hidden-report.txt"));
    touch(&f.p(".config/report.ini"));
    touch(&f.p("proj/node_modules/pkg/report.js"));
    touch(&f.p("proj/main.o"));
    touch(&f.p("proj/target/debug/report-bin"));
    fs::write(f.p("proj/target/CACHEDIR.TAG"), b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();
    f.scan(Mode::Quick);
    assert_eq!(f.find("report"), vec!["docs/report.txt"]);
    assert!(f.indexed("proj/target"), "a tagged cache dir itself is still findable");
    assert!(!f.indexed("proj/target/debug"));
    assert!(!f.indexed("proj/main.o"));
}

#[test]
fn symlinks_are_not_followed_and_loops_are_harmless() {
    let mut f = fx();
    touch(&f.p("a/real-file.txt"));
    std::os::unix::fs::symlink(&f.root, f.p("a/loop")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/target", f.p("a/dangling")).unwrap();
    f.scan(Mode::Full);
    assert_eq!(f.find("real-file"), vec!["a/real-file.txt"]);
    assert!(f.indexed("a/loop"));
    assert!(f.indexed("a/dangling"));
    assert!(!f.indexed("a/loop/a"));
}

#[test]
fn legacy_cached_listings_are_invalidated_on_upgrade() {
    let mut f = fx();
    touch(&f.p("cache/report.txt"));
    let tag = f.p("cache/CACHEDIR.TAG");
    fs::write(&tag, b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();
    f.scan(Mode::Quick);

    // Older engines marked tagged directories reconciled despite skipping their listing.
    let legacy_policy = format!("v1:{:?}:{:?}", f.ix.st.rules, f.ix.st.roots);
    f.ix.w.apply_policy(&legacy_policy).unwrap();
    let meta = ferroseek_index::Meta::from_metadata(&fs::metadata(f.p("cache")).unwrap());
    let mut tx = f.ix.w.transaction().unwrap();
    let id = tx.lookup(&f.root.join("cache")).unwrap().unwrap().0;
    tx.mark_dir_reconciled(id, meta.mtime_ns, meta.ino).unwrap();
    tx.commit().unwrap();

    fs::write(&tag, b"not a cache directory tag").unwrap();
    f.scan(Mode::Quick);
    assert!(f.indexed("cache/report.txt"), "upgrades must recover listings cached by older engines");
}

#[test]
fn create_modify_delete_via_events() {
    let mut f = fx();
    f.scan(Mode::Quick);
    touch(&f.p("new-note.md"));
    f.settle();
    assert_eq!(f.find("new-note"), vec!["new-note.md"]);
    let r = Reader::open(&f.db).unwrap();
    let (_, m1) = r.lookup(&f.p("new-note.md")).unwrap().unwrap();
    fs::write(f.p("new-note.md"), b"much longer content").unwrap();
    f.settle();
    let (_, m2) = r.lookup(&f.p("new-note.md")).unwrap().unwrap();
    assert_ne!(m1.size, m2.size);
    fs::remove_file(f.p("new-note.md")).unwrap();
    f.settle();
    assert!(f.find("new-note").is_empty());
}

#[test]
fn new_nested_directories_are_fully_indexed() {
    let mut f = fx();
    f.scan(Mode::Quick);
    // Created faster than a watch can be added to each level.
    touch(&f.p("x/y/z/deep-file.txt"));
    touch(&f.p("x/y/sibling.txt"));
    f.settle();
    assert_eq!(f.find("deep-file"), vec!["x/y/z/deep-file.txt"]);
    assert_eq!(f.find("sibling"), vec!["x/y/sibling.txt"]);
    // And the new dirs are watched.
    touch(&f.p("x/y/z/later.txt"));
    f.settle();
    assert_eq!(f.find("later"), vec!["x/y/z/later.txt"]);
}

#[test]
fn rename_file_and_directory_keeps_subtree_and_watches() {
    let mut f = fx();
    touch(&f.p("proj/src/lib.rs"));
    f.scan(Mode::Quick);
    fs::rename(f.p("proj"), f.p("project-renamed")).unwrap();
    f.settle();
    assert_eq!(f.find("lib.rs"), vec!["project-renamed/src/lib.rs"]);
    assert!(f.find("proj/").iter().all(|p| p.starts_with("project-renamed")));
    // Watch on the renamed subtree still delivers events.
    touch(&f.p("project-renamed/src/main.rs"));
    f.settle();
    assert_eq!(f.find("main.rs"), vec!["project-renamed/src/main.rs"]);
    fs::rename(f.p("project-renamed/src/main.rs"), f.p("project-renamed/app.rs")).unwrap();
    f.settle();
    assert_eq!(f.find("app.rs"), vec!["project-renamed/app.rs"]);
    assert!(f.find("main.rs").is_empty());
}

#[test]
fn atomic_save_replaces_file() {
    let mut f = fx();
    touch(&f.p("doc.txt"));
    f.scan(Mode::Quick);
    let (_, before) = Reader::open(&f.db).unwrap().lookup(&f.p("doc.txt")).unwrap().unwrap();
    fs::write(f.p(".doc.txt.tmp123"), b"new contents here").unwrap();
    fs::rename(f.p(".doc.txt.tmp123"), f.p("doc.txt")).unwrap();
    f.settle();
    assert_eq!(f.find("doc.txt"), vec!["doc.txt"]);
    let (_, after) = Reader::open(&f.db).unwrap().lookup(&f.p("doc.txt")).unwrap().unwrap();
    assert_ne!(before.ino, after.ino);
    assert_eq!(after.size, 17);
    assert!(f.find("tmp123").is_empty());
}

#[test]
fn rename_rechecks_descendant_path_exclusions() {
    for restart in [false, true] {
        let mut f = fx();
        let mut cfg = Config::default();
        cfg.exclude_paths.push(f.p("destination/nested/private").to_string_lossy().into_owned());
        f.ix.st.rules = Rules::new(&cfg, &[]);
        touch(&f.p("source/nested/private/report.txt"));
        touch(&f.p("source/nested/public/notes.txt"));
        f.scan(Mode::Quick);
        assert!(f.indexed("source/nested/private/report.txt"));

        fs::rename(f.p("source"), f.p("destination")).unwrap();
        if restart {
            assert!(f.ix.pump_events().unwrap() > 0);
            assert!(f.ix.has_work());
            f.ix.into_writer().close().unwrap();
            f.ix = make_ix(&f.db, &f.root, &cfg, true);
            f.scan(Mode::Quick);
        }
        f.settle();
        assert!(!f.indexed("destination/nested/private"));
        assert!(f.find("report").is_empty());
        assert!(f.indexed("destination/nested/public/notes.txt"));

        fs::rename(f.p("destination"), f.p("source")).unwrap();
        f.settle();
        assert!(f.indexed("source/nested/private/report.txt"));
    }
}

#[test]
fn rename_rechecks_depth_limit_and_drops_watches() {
    let mut f = fx_with(Config { max_depth: 1, ..Config::default() });
    touch(&f.p("source/report.txt"));
    touch(&f.p("source/nested/deeper.txt"));
    fs::create_dir(f.p("destination")).unwrap();
    f.scan(Mode::Quick);
    assert!(f.indexed("source/report.txt"));
    assert!(!f.indexed("source/nested/deeper.txt"));

    fs::rename(f.p("source"), f.p("destination/source")).unwrap();
    f.settle();
    assert!(f.indexed("destination/source"));
    assert!(f.find("report").is_empty());
    assert!(!f.indexed("destination/source/nested"));
    assert_eq!(f.ix.watched_dirs(), 2, "only root and destination remain watched");
    touch(&f.p("destination/source/later.txt"));
    f.settle();
    assert!(!f.indexed("destination/source/later.txt"));

    fs::rename(f.p("destination/source"), f.p("source")).unwrap();
    f.settle();
    assert!(f.indexed("source/report.txt"));
    assert!(f.indexed("source/later.txt"));
    assert!(!f.indexed("source/nested/deeper.txt"));
    assert_eq!(f.ix.watched_dirs(), 3);
}

#[test]
fn rename_drops_descendant_watches_beyond_depth_limit() {
    let mut f = fx_with(Config { max_depth: 2, ..Config::default() });
    touch(&f.p("source/nested/report.txt"));
    fs::create_dir(f.p("destination")).unwrap();
    f.scan(Mode::Quick);
    assert!(f.indexed("source/nested/report.txt"));

    fs::rename(f.p("source"), f.p("destination/source")).unwrap();
    f.settle();
    assert!(f.indexed("destination/source/nested"));
    assert!(f.find("report").is_empty());
    assert_eq!(f.ix.watched_dirs(), 3, "only root, destination and source remain watched");

    touch(&f.p("destination/source/nested/later.txt"));
    f.settle();
    assert!(!f.indexed("destination/source/nested/later.txt"));
    fs::rename(f.p("destination/source"), f.p("source")).unwrap();
    f.settle();
    assert!(f.indexed("source/nested/report.txt"));
    assert!(f.indexed("source/nested/later.txt"));
}

#[test]
fn move_out_of_and_into_the_tree() {
    let mut f = fx();
    let outside = f._t.path().join("outside");
    fs::create_dir(&outside).unwrap();
    touch(&f.p("leaving/inner/gone.txt"));
    touch(&outside.join("arriving/inner/welcome.txt"));
    f.scan(Mode::Quick);
    let watched_before = f.ix.watched_dirs();

    fs::rename(f.p("leaving"), outside.join("leaving")).unwrap();
    f.settle();
    assert!(f.find("gone").is_empty());
    assert_eq!(f.ix.watched_dirs(), watched_before - 2, "watches on moved-out dirs dropped");
    // Activity in the moved-out tree must not leak into the index.
    touch(&outside.join("leaving/inner/after-move.txt"));
    f.settle();
    assert!(f.find("after-move").is_empty());

    fs::rename(outside.join("arriving"), f.p("arriving")).unwrap();
    f.settle();
    assert_eq!(f.find("welcome"), vec!["arriving/inner/welcome.txt"]);
}

#[test]
fn rename_into_excluded_name_and_back() {
    let mut f = fx();
    touch(&f.p("visible/file-a.txt"));
    f.scan(Mode::Quick);
    fs::rename(f.p("visible"), f.p(".now-hidden")).unwrap();
    f.settle();
    assert!(f.find("file-a").is_empty());
    fs::rename(f.p(".now-hidden"), f.p("visible-again")).unwrap();
    f.settle();
    assert_eq!(f.find("file-a"), vec!["visible-again/file-a.txt"]);
}

#[test]
fn directory_replaced_by_file_and_back() {
    let mut f = fx();
    touch(&f.p("thing/child-x.txt"));
    f.scan(Mode::Quick);
    fs::remove_dir_all(f.p("thing")).unwrap();
    touch(&f.p("thing"));
    f.settle();
    assert!(f.find("child-x").is_empty());
    assert_eq!(f.find("thing"), vec!["thing"]);
    fs::remove_file(f.p("thing")).unwrap();
    touch(&f.p("thing/child-y.txt"));
    f.settle();
    assert_eq!(f.find("child-y"), vec!["thing/child-y.txt"]);
}

#[test]
fn stale_parent_path_across_batches_loses_nothing() {
    let mut f = fx();
    fs::create_dir(f.p("d")).unwrap();
    f.scan(Mode::Quick);
    touch(&f.p("d/created-before-rename.txt"));
    fs::rename(f.p("d"), f.p("d2")).unwrap();
    // Read everything, then deliver the create in one batch and the rename
    // in the next: the first batch sees a stale parent path.
    let ino = f.ix.st.ino.clone().unwrap();
    let mut evs = Vec::new();
    ino.read_available(&mut vec![0u8; 65536], &mut evs).unwrap();
    let split = evs.iter().position(|e| e.mask & crate::inotify::IN_MOVED_FROM != 0).unwrap();
    let second = evs.split_off(split);
    f.ix.apply_events(evs).unwrap();
    assert!(f.ix.has_retries(), "missing file under a stale parent must not be treated as deleted");
    f.ix.apply_events(second).unwrap();
    f.ix.scan_to_completion().unwrap();
    assert_eq!(f.find("created-before-rename"), vec!["d2/created-before-rename.txt"]);
    assert!(!f.ix.has_retries());
}

#[test]
fn overflow_triggers_reconcile_that_catches_missed_changes() {
    let mut f = fx_with(Config::default());
    touch(&f.p("keep.txt"));
    touch(&f.p("sub/remove-me.txt"));
    f.scan(Mode::Quick);
    // Make changes, then throw the real events away and inject an overflow
    // instead, exactly what the kernel does when its queue is full.
    touch(&f.p("sub/missed-create.txt"));
    fs::remove_file(f.p("sub/remove-me.txt")).unwrap();
    let ino = f.ix.st.ino.clone().unwrap();
    ino.read_available(&mut vec![0u8; 65536], &mut Vec::new()).unwrap();
    f.ix.apply_events(vec![RawEvent { wd: -1, mask: IN_Q_OVERFLOW, cookie: 0, name: vec![] }]).unwrap();
    assert!(f.ix.has_work());
    f.ix.scan_to_completion().unwrap();
    assert_eq!(f.find("missed-create"), vec!["sub/missed-create.txt"]);
    assert!(f.find("remove-me").is_empty());
    assert_eq!(f.ix.st.status.overflows.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn offline_changes_quick_vs_full_reconcile() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("root");
    touch(&root.join("a/file.txt"));
    touch(&root.join("b/other.txt"));
    let db = t.path().join("index.sqlite");
    let cfg = Config::default();
    {
        let mut ix = make_ix(&db, &root, &cfg, false);
        ix.sync_roots(Mode::Quick).unwrap();
        ix.scan_to_completion().unwrap();
        ix.into_writer().close().unwrap();
    }
    // Daemon "down": names change in a/, content changes in b/.
    let before = fs::metadata(root.join("a")).unwrap().modified().unwrap();
    touch(&root.join("a/offline-new.txt"));
    fs::File::open(root.join("a")).unwrap().set_modified(before + std::time::Duration::from_secs(1)).unwrap();
    fs::write(root.join("b/other.txt"), b"changed while offline").unwrap();

    let mut ix = make_ix(&db, &root, &cfg, false);
    ix.sync_roots(Mode::Quick).unwrap();
    ix.scan_to_completion().unwrap();
    let r = Reader::open(&db).unwrap();
    assert!(r.lookup(&root.join("a/offline-new.txt")).unwrap().is_some(), "quick sees name changes");
    let (_, m) = r.lookup(&root.join("b/other.txt")).unwrap().unwrap();
    assert_eq!(m.size, 1, "quick skips unchanged directories (documented)");
    ix.sync_roots(Mode::Full).unwrap();
    ix.scan_to_completion().unwrap();
    let (_, m) = r.lookup(&root.join("b/other.txt")).unwrap().unwrap();
    assert_eq!(m.size, 21, "full re-stats every file");
}

#[test]
fn permission_denied_directory_is_skipped_not_fatal() {
    let mut f = fx();
    touch(&f.p("open/fine.txt"));
    touch(&f.p("locked/secret.txt"));
    fs::set_permissions(f.p("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    f.scan(Mode::Quick);
    fs::set_permissions(f.p("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if f.find("secret").is_empty() {
        assert_eq!(f.find("fine"), vec!["open/fine.txt"]);
        assert!(f.indexed("locked"));
        assert!(f.ix.st.status.errors.load(std::sync::atomic::Ordering::Relaxed) > 0);
    } // else: running with CAP_DAC_OVERRIDE (root); nothing to assert.
}

#[test]
fn non_utf8_and_unicode_names() {
    let mut f = fx();
    f.scan(Mode::Quick);
    let raw = f.root.join(OsStr::from_bytes(b"bad-\xff\xfe-name.txt"));
    fs::write(&raw, b"x").unwrap();
    touch(&f.p("Ünïcödé-ファイル.md"));
    f.settle();
    let r = Reader::open(&f.db).unwrap();
    assert!(r.lookup(&raw).unwrap().is_some());
    assert_eq!(f.find("ünïcödé").len(), 1);
    assert_eq!(f.find("ファイル").len(), 1);
    let hits = r.search(&Query::parse("bad-"), &SearchOptions::default()).unwrap();
    assert_eq!(hits[0].path, raw);
}

#[test]
fn reopen_persists_and_rebuild_is_repeatable() {
    let mut f = fx();
    for i in 0..50 {
        touch(&f.p(&format!("d{}/file-{i}.txt", i % 5)));
    }
    f.scan(Mode::Quick);
    let n = f.count();
    assert_eq!(n, 56, "50 files + 5 dirs + the root");
    let w = std::mem::replace(&mut f.ix, make_ix(&f.db, &f.root, &Config::default(), true)).into_writer();
    w.close().unwrap();
    assert_eq!(f.count(), n);
    for _ in 0..3 {
        f.ix.rebuild().unwrap();
        f.ix.scan_to_completion().unwrap();
        assert_eq!(f.count(), n);
    }
}

#[test]
fn burst_of_events_is_bounded_and_complete() {
    let mut f = fx();
    f.scan(Mode::Quick);
    for i in 0..3000 {
        fs::write(f.p(&format!("burst-{i:05}.dat")), b"x").unwrap();
    }
    f.settle();
    assert_eq!(f.find("burst-").len(), 1000, "search limit");
    assert_eq!(f.count(), 3001);
    for i in 0..3000 {
        fs::rename(f.p(&format!("burst-{i:05}.dat")), f.p(&format!("renamed-{i:05}.dat"))).unwrap();
    }
    f.settle();
    assert!(f.find("burst-").is_empty());
    assert_eq!(f.count(), 3001);
}

#[test]
fn removed_root_is_dropped() {
    let mut f = fx();
    touch(&f.p("x.txt"));
    f.scan(Mode::Quick);
    assert_eq!(f.count(), 2);
    let w = std::mem::replace(&mut f.ix, make_ix(&f.db, &f.root, &Config::default(), false)).into_writer();
    drop(w);
    let (w, _) = Writer::open(&f.db).unwrap();
    let other = f._t.path().join("other-root");
    fs::create_dir(&other).unwrap();
    let mut ix = Indexer::new(w, Rules::new(&Config::default(), &[]), vec![other], None, Arc::new(Status::default()));
    ix.sync_roots(Mode::Quick).unwrap();
    ix.scan_to_completion().unwrap();
    assert_eq!(f.count(), 1);
}

#[test]
fn cache_tag_blocks_live_files_and_removal_restores_existing_files() {
    let mut f = fx();
    touch(&f.p("cache/old-document.txt"));
    fs::write(f.p("cache/CACHEDIR.TAG"), b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();
    f.scan(Mode::Quick);
    touch(&f.p("cache/new-document.txt"));
    f.settle();
    assert!(f.find("document").is_empty());
    fs::remove_file(f.p("cache/CACHEDIR.TAG")).unwrap();
    f.settle();
    assert_eq!(f.find("document").len(), 2);
}

#[test]
fn transient_stat_errors_preserve_entries_and_retry_listing() {
    for errno in [libc::EIO, libc::ESTALE, libc::EACCES] {
        let mut f = fx();
        touch(&f.p("folder/keep.txt"));
        f.scan(Mode::Full);
        f.ix.st.stat_failures.insert(f.p("folder"), errno);
        f.scan(Mode::Full);
        assert!(f.indexed("folder/keep.txt"), "errno {errno} removed a subtree");
        f.ix.st.stat_failures.clear();
        // A failed new child's stat must also invalidate an otherwise unchanged listing.
        touch(&f.p("new.txt"));
        f.ix.st.stat_failures.insert(f.p("new.txt"), errno);
        f.scan(Mode::Full);
        f.ix.st.stat_failures.clear();
        f.scan(Mode::Quick);
        assert!(f.indexed("new.txt"), "errno {errno} permanently skipped a new file");
    }
}

#[test]
fn incomplete_readdir_preserves_unseen_children_until_successful_retry() {
    let mut f = fx();
    for name in ["a.txt", "b.txt", "c.txt"] {
        touch(&f.p(name));
    }
    f.scan(Mode::Full);
    f.ix.st.listing_failure = Some((f.root.clone(), 1));
    f.scan(Mode::Full);
    for name in ["a.txt", "b.txt", "c.txt"] {
        assert!(f.indexed(name));
    }
    f.ix.st.listing_failure = None;
    fs::remove_file(f.p("b.txt")).unwrap();
    f.scan(Mode::Quick);
    assert!(!f.indexed("b.txt"));
    assert!(f.indexed("a.txt") && f.indexed("c.txt"));
}

#[test]
fn transient_event_stat_error_retries_without_another_filesystem_event() {
    let mut f = fx();
    touch(&f.p("file.txt"));
    f.scan(Mode::Full);
    fs::write(f.p("file.txt"), b"longer content").unwrap();
    f.ix.st.stat_failures.insert(f.p("file.txt"), libc::EIO);
    f.ix.pump_events().unwrap();
    assert!(f.indexed("file.txt"));
    assert!(f.ix.has_retries());
    f.ix.st.stat_failures.clear();
    f.ix.idle_tick().unwrap();
    f.ix.scan_to_completion().unwrap();
    let (_, meta) = Reader::open(&f.db).unwrap().lookup(&f.p("file.txt")).unwrap().unwrap();
    assert_eq!(meta.size, 14);
}

#[test]
fn overflow_recovers_metadata_even_when_directory_mtime_did_not_change() {
    let mut f = fx();
    touch(&f.p("unchanged/file.txt"));
    f.scan(Mode::Full);
    let before = fs::metadata(f.p("unchanged")).unwrap().modified().unwrap();
    fs::write(f.p("unchanged/file.txt"), b"changed without renaming").unwrap();
    assert_eq!(before, fs::metadata(f.p("unchanged")).unwrap().modified().unwrap());
    // Discard every real event; only the overflow notification reaches the indexer.
    let ino = f.ix.st.ino.clone().unwrap();
    loop {
        let mut evs = Vec::new();
        ino.read_available(&mut vec![0; 65536], &mut evs).unwrap();
        if evs.is_empty() {
            break;
        }
    }
    f.ix.apply_events(vec![RawEvent { wd: -1, mask: IN_Q_OVERFLOW, cookie: 0, name: vec![] }]).unwrap();
    f.ix.scan_to_completion().unwrap();
    let (_, meta) = Reader::open(&f.db).unwrap().lookup(&f.p("unchanged/file.txt")).unwrap().unwrap();
    assert_eq!(meta.size, 24);
}
