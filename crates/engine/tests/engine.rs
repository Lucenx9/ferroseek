// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end tests of the threaded engine. Temp dirs only.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ferroseek_engine::{Config, Engine, EngineError, Paths, index_once};
use ferroseek_index::{Query, Reader, SearchOptions};

fn setup() -> (tempfile::TempDir, PathBuf, Paths, Config) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("home");
    fs::create_dir(&root).unwrap();
    let paths = Paths { config_file: t.path().join("config.toml"), data_dir: t.path().join("data") };
    let cfg = Config {
        roots: vec![root.to_string_lossy().into_owned()],
        debounce_ms: 20,
        max_cpu_percent: 100,
        ..Config::default()
    };
    (t, root, paths, cfg)
}

fn hits(paths: &Paths, q: &str) -> usize {
    match Reader::open(&paths.index_file()) {
        Ok(r) => r.search(&Query::parse(q), &SearchOptions { limit: 10_000, ..Default::default() }).unwrap().len(),
        Err(_) => 0,
    }
}

fn eventually(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for: {what}");
}

fn write(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"hello").unwrap();
}

#[test]
fn live_updates_pause_resume_restart() {
    let (_t, root, paths, cfg) = setup();
    write(&root.join("existing/alpha-doc.txt"));
    let e = Engine::start(&cfg, &paths).unwrap();
    assert!(e.wait_idle(Duration::from_secs(10)));
    assert_eq!(hits(&paths, "alpha-doc"), 1);
    assert_eq!(e.status().files, 1);

    write(&root.join("existing/beta-live.txt"));
    eventually("live create indexed", || hits(&paths, "beta-live") == 1);
    fs::remove_file(root.join("existing/beta-live.txt")).unwrap();
    eventually("live delete applied", || hits(&paths, "beta-live") == 0);

    e.pause();
    eventually("paused", || e.status().state == "paused");
    write(&root.join("while-paused.txt"));
    fs::write(root.join("existing/alpha-doc.txt"), b"modified while paused").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(hits(&paths, "while-paused"), 0, "paused engine does not index");
    e.resume();
    eventually("indexed after resume", || hits(&paths, "while-paused") == 1);

    eventually("metadata recovered after resume", || {
        Reader::open(&paths.index_file())
            .unwrap()
            .lookup(&root.join("existing/alpha-doc.txt"))
            .unwrap()
            .is_some_and(|(_, meta)| meta.size == 21)
    });

    // Single writer per index.
    assert!(matches!(Engine::start(&cfg, &paths), Err(EngineError::Busy)));
    assert!(matches!(index_once(&cfg, &paths, false, false), Err(EngineError::Busy)));

    e.shutdown();
    // Changes while stopped are picked up by the startup reconcile.
    write(&root.join("existing/offline-gamma.txt"));
    let e = Engine::start(&cfg, &paths).unwrap();
    assert!(e.wait_idle(Duration::from_secs(10)));
    assert_eq!(hits(&paths, "offline-gamma"), 1);
    assert_eq!(hits(&paths, "alpha-doc"), 1);
    e.shutdown();
}

#[test]
fn concurrent_queries_during_indexing() {
    let (_t, root, paths, cfg) = setup();
    for d in 0..40 {
        for f in 0..50 {
            write(&root.join(format!("dir{d:02}/file-{d:02}-{f:02}.txt")));
        }
    }
    let e = Engine::start(&cfg, &paths).unwrap();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let p = paths.clone();
            std::thread::spawn(move || {
                let mut last = 0;
                let end = Instant::now() + Duration::from_secs(10);
                while Instant::now() < end {
                    let n = hits(&p, "file-");
                    assert!(n >= last, "results never go backwards during an initial scan");
                    last = n;
                    if n == 2000 {
                        break;
                    }
                }
                last
            })
        })
        .collect();
    for r in readers {
        assert_eq!(r.join().unwrap(), 2000);
    }
    assert!(e.wait_idle(Duration::from_secs(10)));
    e.shutdown();
}

#[test]
fn rebuild_and_reindex_commands() {
    let (_t, root, paths, cfg) = setup();
    write(&root.join("a/one.txt"));
    let e = Engine::start(&cfg, &paths).unwrap();
    assert!(e.wait_idle(Duration::from_secs(10)));
    for _ in 0..3 {
        e.rebuild();
        std::thread::sleep(Duration::from_millis(50));
        assert!(e.wait_idle(Duration::from_secs(10)));
        assert_eq!(hits(&paths, "one.txt"), 1);
    }
    e.reindex(true);
    assert!(e.wait_idle(Duration::from_secs(10)));
    assert_eq!(e.status().files, 1);
    e.shutdown();
}

#[test]
fn damaged_index_is_recovered_on_start() {
    let (_t, root, paths, cfg) = setup();
    write(&root.join("doc-recover.txt"));
    paths.ensure_data_dir().unwrap();
    fs::write(paths.index_file(), vec![0xABu8; 8192]).unwrap();
    let e = Engine::start(&cfg, &paths).unwrap();
    assert!(e.wait_idle(Duration::from_secs(10)));
    assert_eq!(hits(&paths, "doc-recover"), 1);
    assert!(paths.data_dir.join("index.sqlite.broken").exists());
    e.shutdown();
}

#[test]
fn index_once_without_daemon() {
    let (_t, root, paths, cfg) = setup();
    write(&root.join("x/once.txt"));
    let s = index_once(&cfg, &paths, false, false).unwrap();
    assert_eq!(s.files, 1);
    assert_eq!(hits(&paths, "once"), 1);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&paths.data_dir).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn changed_exclusions_apply_on_restart_without_filesystem_changes() {
    let (_t, root, paths, mut cfg) = setup();
    write(&root.join("private/secret-note.txt"));
    index_once(&cfg, &paths, false, false).unwrap();
    assert_eq!(hits(&paths, "secret-note"), 1);
    cfg.exclude_names.push("private".into());
    index_once(&cfg, &paths, false, false).unwrap();
    assert_eq!(hits(&paths, "secret-note"), 0);
    cfg.exclude_names.pop();
    let e = Engine::start(&cfg, &paths).unwrap();
    eventually("newly included directory indexed", || hits(&paths, "secret-note") == 1);
    e.shutdown();
}

#[test]
fn invalidated_cache_tag_is_rechecked_on_quick_restart() {
    for relative in ["", "cache"] {
        let (_t, root, paths, cfg) = setup();
        let cached = root.join(relative);
        write(&cached.join("report.txt"));
        let tag = cached.join("CACHEDIR.TAG");
        fs::write(&tag, b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();

        index_once(&cfg, &paths, false, false).unwrap();
        assert_eq!(hits(&paths, "report"), 0, "valid cache tags exclude their contents");
        index_once(&cfg, &paths, false, false).unwrap();
        assert_eq!(hits(&paths, "report"), 0, "unchanged tags remain excluded after restart");

        let directory_mtime = fs::metadata(&cached).unwrap().modified().unwrap();
        fs::write(&tag, b"not a cache directory tag").unwrap();
        assert_eq!(fs::metadata(&cached).unwrap().modified().unwrap(), directory_mtime);

        index_once(&cfg, &paths, false, false).unwrap();
        assert_eq!(hits(&paths, "report"), 1, "invalidated tags must reveal contents on quick restart: {relative:?}");
    }
}

#[test]
fn cache_tag_activated_by_events_is_rechecked_on_quick_restart() {
    let (_t, root, paths, cfg) = setup();
    let cached = root.join("cache");
    write(&cached.join("report.txt"));
    let tag = cached.join("CACHEDIR.TAG");
    fs::write(&tag, b"not a cache directory tag").unwrap();
    let engine = Engine::start(&cfg, &paths).unwrap();
    assert!(engine.wait_idle(Duration::from_secs(10)));
    assert_eq!(hits(&paths, "report"), 1);

    let directory_mtime = fs::metadata(&cached).unwrap().modified().unwrap();
    fs::write(&tag, b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();
    eventually("cache tag activation removes contents", || hits(&paths, "report") == 0);
    engine.shutdown();

    fs::write(&tag, b"not a cache directory tag").unwrap();
    assert_eq!(fs::metadata(&cached).unwrap().modified().unwrap(), directory_mtime);
    index_once(&cfg, &paths, false, false).unwrap();
    assert_eq!(hits(&paths, "report"), 1, "event-driven pruning must not cache an empty listing across restarts");
}
