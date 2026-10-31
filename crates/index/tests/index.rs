// SPDX-License-Identifier: GPL-3.0-or-later
//! Integration tests for the index through its public interface only.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use ferroseek_index::{Change, Kind, Meta, OpenOutcome, Query, Reader, SearchOptions, Writer};

fn file(ino: u64) -> Meta {
    Meta { kind: Kind::File, size: 10, mtime_ns: 1_000, ino }
}
fn dir(ino: u64) -> Meta {
    Meta { kind: Kind::Dir, size: 0, mtime_ns: 5_000, ino }
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("index.sqlite");
    (t, db)
}

fn names(hits: &[ferroseek_index::Hit]) -> Vec<String> {
    hits.iter().map(|h| h.path.file_name().unwrap().to_string_lossy().into_owned()).collect()
}

fn search(db: &Path, q: &str) -> Vec<ferroseek_index::Hit> {
    Reader::open(db).unwrap().search(&Query::parse(q), &SearchOptions::default()).unwrap()
}

#[test]
fn create_reopen_and_clean_shutdown() {
    let (_t, db) = setup();
    let (w, o) = Writer::open(&db).unwrap();
    assert_eq!(o, OpenOutcome::Created);
    w.close().unwrap();
    let (_w, o) = Writer::open(&db).unwrap();
    assert_eq!(o, OpenOutcome::Existing);
}

#[test]
fn db_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, db) = setup();
    let _ = Writer::open(&db).unwrap();
    assert_eq!(std::fs::metadata(&db).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn garbage_database_is_moved_aside_and_recreated() {
    let (_t, db) = setup();
    std::fs::write(&db, b"this is definitely not an sqlite database, just junk bytes....").unwrap();
    let (mut w, o) = Writer::open(&db).unwrap();
    match o {
        OpenOutcome::Recovered { moved_to, .. } => assert!(moved_to.exists()),
        other => panic!("expected recovery, got {other:?}"),
    }
    // Usable afterwards.
    let mut tx = w.transaction().unwrap();
    tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    tx.commit().unwrap();
}

#[test]
fn foreign_sqlite_database_is_not_used() {
    let (_t, db) = setup();
    let c = rusqlite_conn(&db);
    c.execute_batch("CREATE TABLE something(x); INSERT INTO something VALUES(1);").unwrap();
    drop(c);
    let (_w, o) = Writer::open(&db).unwrap();
    assert!(matches!(o, OpenOutcome::Recovered { .. }));
}

fn rusqlite_conn(p: &Path) -> rusqlite_shim::Connection {
    rusqlite_shim::Connection::open(p).unwrap()
}

// The index crate re-exports nothing from rusqlite; tests that need raw
// SQLite access use the same crate via the dev-dependency graph.
mod rusqlite_shim {
    pub use ::rusqlite::Connection;
}

#[test]
fn upsert_insert_update_unchanged() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    assert_eq!(tx.ensure_root(Path::new("/r"), dir(1)).unwrap(), root);
    let (id, c) = tx.upsert(root, b"a.txt", file(2)).unwrap();
    assert_eq!(c, Change::Inserted);
    assert_eq!(tx.upsert(root, b"a.txt", file(2)).unwrap(), (id, Change::Unchanged));
    let mut m = file(2);
    m.size = 99;
    assert_eq!(tx.upsert(root, b"a.txt", m).unwrap(), (id, Change::Updated));
    assert_eq!(tx.meta(id).unwrap().unwrap().size, 99);
    tx.commit().unwrap();
}

#[test]
fn dir_mtime_only_changes_via_mark_reconciled() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    let (d, _) = tx.upsert(root, b"d", dir(2)).unwrap();
    assert_eq!(tx.meta(d).unwrap().unwrap().mtime_ns, 0, "new dirs are unreconciled");
    tx.mark_dir_reconciled(d, 777, 2).unwrap();
    let mut m = dir(2);
    m.mtime_ns = 999;
    assert_eq!(tx.upsert(root, b"d", m).unwrap().1, Change::Unchanged);
    assert_eq!(tx.meta(d).unwrap().unwrap().mtime_ns, 777);
    // Replaced by a different directory inode: reset to unreconciled.
    tx.upsert(root, b"d", dir(3)).unwrap();
    assert_eq!(tx.meta(d).unwrap().unwrap().mtime_ns, 0);
    tx.commit().unwrap();
}

#[test]
fn delete_removes_subtree_and_search_rows() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    let (d, _) = tx.upsert(root, b"projects", dir(2)).unwrap();
    let (d2, _) = tx.upsert(d, b"inner", dir(3)).unwrap();
    tx.upsert(d2, b"deepreport.txt", file(4)).unwrap();
    tx.upsert(root, b"keepreport.txt", file(5)).unwrap();
    tx.commit().unwrap();
    assert_eq!(names(&search(&db, "report")).len(), 2);

    let mut tx = w.transaction().unwrap();
    assert_eq!(tx.subtree_dirs(d).unwrap().len(), 2);
    assert_eq!(tx.delete(d).unwrap(), 3);
    assert_eq!(tx.delete(d).unwrap(), 0);
    tx.commit().unwrap();
    assert_eq!(names(&search(&db, "report")), vec!["keepreport.txt"]);
}

#[test]
fn file_replacing_directory_drops_its_children() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    let (d, _) = tx.upsert(root, b"thing", dir(2)).unwrap();
    tx.upsert(d, b"child-unique.txt", file(3)).unwrap();
    tx.upsert(root, b"thing", file(4)).unwrap();
    assert!(tx.children(d).unwrap().is_empty());
    tx.commit().unwrap();
    assert!(search(&db, "child-unique").is_empty());
}

#[test]
fn rename_moves_subtree_in_constant_rows_and_replaces_destination() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    let (a, _) = tx.upsert(root, b"alpha", dir(2)).unwrap();
    let (b, _) = tx.upsert(root, b"beta", dir(3)).unwrap();
    let (f, _) = tx.upsert(a, b"notes.md", file(4)).unwrap();
    tx.upsert(b, b"old.md", file(5)).unwrap();
    tx.commit().unwrap();

    let mut tx = w.transaction().unwrap();
    let before = tx.ops();
    tx.rename(a, b, b"gamma").unwrap();
    assert_eq!(tx.ops() - before, 1, "renaming a directory touches one row");
    assert_eq!(tx.path(f).unwrap().unwrap(), Path::new("/r/beta/gamma/notes.md"));
    // Atomic-save style: rename over an existing file replaces it.
    let (tmp, _) = tx.upsert(b, b".old.md.swp", file(6)).unwrap();
    tx.rename(tmp, b, b"old.md").unwrap();
    assert_eq!(tx.children(b).unwrap().len(), 2);
    assert_eq!(tx.child(b, b"old.md").unwrap().unwrap().id, tmp);
    tx.commit().unwrap();

    assert!(search(&db, "alpha").is_empty());
    let h = search(&db, "notes");
    assert_eq!(h[0].path, Path::new("/r/beta/gamma/notes.md"));
}

#[test]
fn non_utf8_names_round_trip_exactly() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let raw: &[u8] = b"caf\xe9-latin1-report.txt";
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    tx.upsert(root, raw, file(2)).unwrap();
    tx.commit().unwrap();
    let h = search(&db, "latin1");
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].path.file_name().unwrap().as_bytes(), raw);
    let r = Reader::open(&db).unwrap();
    let mut p = PathBuf::from("/r");
    p.push(OsStr::from_bytes(raw));
    assert!(r.lookup(&p).unwrap().is_some());
}

#[test]
fn unicode_case_insensitive() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    tx.upsert(root, "ÜBERSICHT-Straße.PDF".as_bytes(), file(2)).unwrap();
    tx.upsert(root, "日本語のファイル.txt".as_bytes(), file(3)).unwrap();
    tx.commit().unwrap();
    assert_eq!(search(&db, "übersicht").len(), 1);
    assert_eq!(search(&db, "straße.pdf").len(), 1);
    assert_eq!(search(&db, "のファイ").len(), 1);
}

fn ranked_fixture() -> (tempfile::TempDir, PathBuf) {
    let (t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/home/u"), dir(1)).unwrap();
    let (docs, _) = tx.upsert(root, b"Documents", dir(2)).unwrap();
    let (deep, _) = tx.upsert(docs, b"a", dir(3)).unwrap();
    let (deeper, _) = tx.upsert(deep, b"b", dir(4)).unwrap();
    let (hid, _) = tx.upsert(root, b".cache", dir(5)).unwrap();
    for (p, n, i) in [
        (docs, "report.pdf", 10),
        (docs, "Report", 11),
        (docs, "annual-report-2024.pdf", 12),
        (docs, "myReportDraft.odt", 13),
        (docs, "unreported.txt", 14),
        (deeper, "report.pdf.bak", 15),
        (hid, "report.cache", 16),
        (docs, "cv.pdf", 17),
        (docs, "cvs-notes.txt", 18),
    ] {
        tx.upsert(p, n.as_bytes(), file(i)).unwrap();
    }
    tx.commit().unwrap();
    (t, db)
}

#[test]
fn ranking_prefers_exact_then_stem_then_prefix_then_boundary() {
    let (_t, db) = ranked_fixture();
    let n = names(&search(&db, "report"));
    assert_eq!(n[0], "Report", "{n:?}");
    assert_eq!(n[1], "report.pdf", "{n:?}");
    let pos = |x: &str| n.iter().position(|v| v == x).unwrap();
    assert!(pos("annual-report-2024.pdf") < pos("unreported.txt"), "{n:?}");
    assert!(pos("myReportDraft.odt") < pos("unreported.txt"), "camelCase boundary: {n:?}");
    assert!(pos("report.pdf.bak") < pos("unreported.txt"), "{n:?}");
}

#[test]
fn short_queries_match_prefixes() {
    let (_t, db) = ranked_fixture();
    let n = names(&search(&db, "cv"));
    assert_eq!(n, vec!["cv.pdf", "cvs-notes.txt"]);
}

#[test]
fn multi_token_and_path_tokens() {
    let (_t, db) = ranked_fixture();
    assert_eq!(names(&search(&db, "report 2024")), vec!["annual-report-2024.pdf"]);
    assert_eq!(names(&search(&db, "pdf report")).len(), 3);
    let h = search(&db, "a/b/ report");
    assert_eq!(names(&h), vec!["report.pdf.bak"]);
    let h = search(&db, "documents/cv");
    assert_eq!(names(&h), vec!["cv.pdf", "cvs-notes.txt"]);
}

#[test]
fn options_within_kind_hidden_limit() {
    let (_t, db) = ranked_fixture();
    let r = Reader::open(&db).unwrap();
    let q = Query::parse("report");
    let opts = SearchOptions { include_hidden: false, ..Default::default() };
    assert!(!names(&r.search(&q, &opts).unwrap()).contains(&"report.cache".to_string()));
    let opts = SearchOptions { within: Some("/home/u/Documents/a".into()), ..Default::default() };
    assert_eq!(names(&r.search(&q, &opts).unwrap()), vec!["report.pdf.bak"]);
    let opts = SearchOptions { limit: 2, ..Default::default() };
    assert_eq!(r.search(&q, &opts).unwrap().len(), 2);
    let opts = SearchOptions { kind: Some(Kind::Dir), ..Default::default() };
    assert!(r.search(&Query::parse("docu"), &opts).unwrap().len() == 1);
    assert!(r.search(&Query::parse(""), &SearchOptions::default()).unwrap().is_empty());
}

#[test]
fn quote_and_fts_syntax_in_queries_is_literal() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    tx.upsert(root, br#"say "hi" AND NOT bye*.txt"#, file(2)).unwrap();
    tx.commit().unwrap();
    for q in [r#""hi""#, "AND", "bye*", "NOT", "(", "\"", "*", "-", "^x", "NEAR(a b)"] {
        let _ = search(&db, q); // must not error
    }
    assert_eq!(search(&db, r#""hi""#).len(), 1);
    assert_eq!(search(&db, "bye*").len(), 1);
}

#[test]
fn reader_sees_writer_commits_and_not_uncommitted() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let r = Reader::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    tx.upsert(root, b"pending-file.txt", file(2)).unwrap();
    let q = Query::parse("pending");
    assert!(r.search(&q, &SearchOptions::default()).unwrap().is_empty());
    tx.commit().unwrap();
    assert_eq!(r.search(&q, &SearchOptions::default()).unwrap().len(), 1);
}

#[test]
fn stats_counts() {
    let (_t, db) = ranked_fixture();
    let s = Reader::open(&db).unwrap().stats().unwrap();
    assert_eq!(s.files, 9);
    assert_eq!(s.dirs, 5);
    assert_eq!(s.roots, vec![PathBuf::from("/home/u")]);
    assert!(s.disk_bytes > 0);
}

#[test]
fn root_slash_paths_are_well_formed() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/"), dir(1)).unwrap();
    let (e, _) = tx.upsert(root, b"etc", dir(2)).unwrap();
    tx.upsert(e, b"hostname-file", file(3)).unwrap();
    tx.commit().unwrap();
    assert_eq!(search(&db, "hostname")[0].path, Path::new("/etc/hostname-file"));
}

#[test]
fn hidden_and_depth_are_relative_to_the_root() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    // The root itself lives below a dot-directory.
    let root = tx.ensure_root(Path::new("/tmp/.private/root"), dir(1)).unwrap();
    tx.upsert(root, b"visible-thing.txt", file(2)).unwrap();
    let (h, _) = tx.upsert(root, b".dot", dir(3)).unwrap();
    tx.upsert(h, b"hidden-thing.txt", file(4)).unwrap();
    tx.commit().unwrap();
    let r = Reader::open(&db).unwrap();
    let opts = SearchOptions { include_hidden: false, ..Default::default() };
    assert_eq!(names(&r.search(&Query::parse("thing"), &opts).unwrap()), vec!["visible-thing.txt"]);
}

#[test]
fn large_limits_see_every_match() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    for i in 0..5000u64 {
        tx.upsert(root, format!("many-{i:05}.txt").as_bytes(), file(10 + i)).unwrap();
    }
    tx.commit().unwrap();
    let r = Reader::open(&db).unwrap();
    let q = Query::parse("many-");
    assert_eq!(r.search(&q, &SearchOptions { limit: 10_000, ..Default::default() }).unwrap().len(), 5000);
    assert_eq!(r.search(&q, &SearchOptions { limit: 15, ..Default::default() }).unwrap().len(), 15);
}

#[test]
fn slash_only_queries_do_not_panic() {
    let (_t, db) = setup();
    let (_w, _) = Writer::open(&db).unwrap();
    for q in ["/", "//", "/// ///"] {
        assert!(search(&db, q).is_empty());
    }
}

#[test]
fn filters_select_candidates_before_applying_the_budget() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    for i in 0..2100 {
        tx.upsert(root, format!("a-report-{i}").as_bytes(), file(i + 2)).unwrap();
    }
    let target = tx.upsert(root, b"target", dir(3000)).unwrap().0;
    tx.upsert(target, b"z-report.txt", file(3001)).unwrap();
    tx.commit().unwrap();
    let r = Reader::open(&db).unwrap();
    let opts = SearchOptions { within: Some(PathBuf::from("/r/target")), ..Default::default() };
    assert_eq!(names(&r.search(&Query::parse("report"), &opts).unwrap()), ["z-report.txt"]);
}

#[test]
fn path_only_queries_find_descendants_without_requiring_parent_name_in_filename() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    let src = tx.upsert(root, b"src", dir(2)).unwrap().0;
    tx.upsert(src, b"main.rs", file(3)).unwrap();
    tx.commit().unwrap();
    assert_eq!(names(&search(&db, "src/")), ["main.rs"]);
    assert_eq!(names(&search(&db, "r/src/")), ["main.rs"]);
}

#[test]
fn path_scoping_matches_independent_paths_across_tree_mutations() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/outside/Root"), dir(1)).unwrap();
    let parent = tx.upsert(root, b"project-src", dir(2)).unwrap().0;
    let nested = tx.upsert(parent, "Ünicode's".as_bytes(), dir(3)).unwrap().0;
    let hidden = tx.upsert(parent, b".hidden", dir(4)).unwrap().0;
    let mut paths = vec![
        PathBuf::from("/outside/Root/project-src/main.rs"),
        PathBuf::from("/outside/Root/project-src/Ünicode's/report.txt"),
        PathBuf::from("/outside/Root/project-src/.hidden/report.txt"),
        PathBuf::from("/outside/Root/project-src/link"),
    ];
    tx.upsert(parent, b"main.rs", file(5)).unwrap();
    tx.upsert(nested, b"report.txt", file(6)).unwrap();
    tx.upsert(hidden, b"report.txt", file(7)).unwrap();
    let mut symlink = file(8);
    symlink.kind = Kind::Symlink;
    tx.upsert(parent, b"link", symlink).unwrap();
    tx.upsert(nested, b"raw-\xff.txt", file(9)).unwrap();
    paths.push(Path::new("/outside/Root/project-src/Ünicode's").join(OsStr::from_bytes(b"raw-\xff.txt")));
    tx.commit().unwrap();
    let check = |expected_paths: &[PathBuf]| {
        let reader = Reader::open(&db).unwrap();
        for query in [
            "src/",
            "project-src/",
            "outside/",
            "root/project",
            "ünicode's/",
            "src/main",
            ".hidden/",
            "missing/",
            "ide's/raw",
            "s/",
            "//",
        ] {
            for include_hidden in [false, true] {
                let opts = SearchOptions { limit: 10000, include_hidden, ..Default::default() };
                let mut actual: Vec<_> = reader
                    .search(&Query::parse(query), &opts)
                    .unwrap()
                    .into_iter()
                    .filter(|h| h.kind != Kind::Dir)
                    .map(|h| h.path)
                    .collect();
                let mut expected: Vec<_> = expected_paths
                    .iter()
                    .filter(|p| {
                        p.to_string_lossy().to_lowercase().contains(query)
                            && (include_hidden
                                || !p.components().any(|c| c.as_os_str().as_bytes().first() == Some(&b'.')))
                    })
                    .cloned()
                    .collect();
                actual.sort();
                expected.sort();
                assert_eq!(actual, expected, "query={query}, hidden={include_hidden}");
            }
        }
    };
    check(&paths);
    let mut tx = w.transaction().unwrap();
    tx.rename(parent, root, b"moved-src").unwrap();
    tx.commit().unwrap();
    for p in &mut paths {
        *p = Path::new("/outside/Root/moved-src").join(p.strip_prefix("/outside/Root/project-src").unwrap());
    }
    check(&paths);
    let mut tx = w.transaction().unwrap();
    tx.delete(parent).unwrap();
    tx.commit().unwrap();
    check(&[]);
}

#[test]
fn within_above_roots_and_large_scoped_results() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/scope/nested"), dir(1)).unwrap();
    let d = tx.upsert(root, b"source", dir(2)).unwrap().0;
    for i in 0..3000 {
        tx.upsert(d, format!("file-{i}.txt").as_bytes(), file(i + 3)).unwrap();
    }
    tx.commit().unwrap();
    let reader = Reader::open(&db).unwrap();
    for within in ["/scope", "/scope/nested", "/scope/nested/source"] {
        let hits = reader
            .search(
                &Query::parse("source/ file-"),
                &SearchOptions { limit: 4000, within: Some(within.into()), ..Default::default() },
            )
            .unwrap();
        assert_eq!(hits.len(), 3000);
    }
}

#[test]
fn searches_observe_one_snapshot_during_directory_renames() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/snapshot"), dir(1)).unwrap();
    let d = tx.upsert(root, b"left", dir(2)).unwrap().0;
    for i in 0..100 {
        tx.upsert(d, format!("report-{i}").as_bytes(), file(i + 3)).unwrap();
    }
    tx.commit().unwrap();
    let handle = std::thread::spawn(move || {
        for i in 0..100 {
            let mut tx = w.transaction().unwrap();
            tx.rename(d, root, if i % 2 == 0 { b"right" } else { b"left" }).unwrap();
            tx.commit().unwrap();
        }
        w.close().unwrap();
    });
    let reader = Reader::open(&db).unwrap();
    for _ in 0..100 {
        let hits = reader
            .search(&Query::parse("snapshot/ report"), &SearchOptions { limit: 200, ..Default::default() })
            .unwrap();
        assert_eq!(hits.len(), 100);
        let parent = hits[0].path.parent().unwrap();
        assert!(hits.iter().all(|h| h.path.parent() == Some(parent)));
    }
    handle.join().unwrap();
}

#[test]
fn scoped_prefix_budget_preserves_exact_stem_ranking() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/rank"), dir(1)).unwrap();
    let source = tx.upsert(root, b"source", dir(2)).unwrap().0;
    let early = tx.upsert(source, b"early", dir(3)).unwrap().0;
    let late = tx.upsert(source, b"late", dir(4)).unwrap().0;
    for i in 0..350 {
        tx.upsert(early, format!("abzzzz{i:04}").as_bytes(), file(i + 10)).unwrap();
    }
    for i in 0..4100 {
        tx.upsert(early, format!("unrelated{i}").as_bytes(), file(i + 1000)).unwrap();
    }
    tx.upsert(late, b"ab.txt", file(6000)).unwrap();
    tx.commit().unwrap();
    let hits = Reader::open(&db)
        .unwrap()
        .search(&Query::parse("source/ ab"), &SearchOptions { limit: 1, ..Default::default() })
        .unwrap();
    assert_eq!(names(&hits), ["ab.txt"]);
    assert!(search(&db, "source/\0 ab").is_empty());
}

#[test]
fn reader_roots_lists_root_paths_only() {
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let mut tx = w.transaction().unwrap();
    let a = tx.ensure_root(Path::new("/srv/a"), dir(1)).unwrap();
    tx.ensure_root(Path::new("/srv/b"), dir(2)).unwrap();
    tx.upsert(a, b"child", dir(3)).unwrap();
    tx.commit().unwrap();
    let mut roots = Reader::open(&db).unwrap().roots().unwrap();
    roots.sort();
    assert_eq!(roots, vec![PathBuf::from("/srv/a"), PathBuf::from("/srv/b")]);
}

#[test]
fn accept_only_searches_return_the_true_top_k() {
    // Review regression: truncating the name ranking before depth/recency
    // adjustments dropped the best final result. 70 equal names deep in the
    // tree (recent) vs one at the root (old, highest id): the root one must
    // win, exactly as the full ranking says.
    let (_t, db) = setup();
    let (mut w, _) = Writer::open(&db).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64;
    let mut tx = w.transaction().unwrap();
    let root = tx.ensure_root(Path::new("/r"), dir(1)).unwrap();
    let mut ino = 10;
    for i in 0..70 {
        let mut parent = root;
        for d in 0..12 {
            ino += 1;
            parent = tx.upsert(parent, format!("b{i}-{d}").as_bytes(), dir(ino)).unwrap().0;
        }
        ino += 1;
        tx.upsert(parent, b"needle.txt", Meta { kind: Kind::File, size: 1, mtime_ns: now, ino }).unwrap();
    }
    tx.upsert(root, b"needle.txt", Meta { kind: Kind::File, size: 1, mtime_ns: 1_000, ino: 99_999 }).unwrap();
    tx.commit().unwrap();
    let r = Reader::open(&db).unwrap();
    let accept = Some(ferroseek_index::PathFilter(std::sync::Arc::new(|_: &Path| true)));
    let q = Query::parse("needle");
    let full = r.search(&q, &SearchOptions { limit: 1000, accept: accept.clone(), ..Default::default() }).unwrap();
    assert_eq!(full.len(), 71);
    for k in [1, 2, 5, 20] {
        let top = r.search(&q, &SearchOptions { limit: k, accept: accept.clone(), ..Default::default() }).unwrap();
        assert_eq!(top, full[..k].to_vec(), "top-{k} must equal the head of the full ranking");
    }
    assert_eq!(full[0].path, Path::new("/r/needle.txt"));
}
