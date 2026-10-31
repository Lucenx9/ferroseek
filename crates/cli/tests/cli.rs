// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs the real `ferroseek` binary against temp dirs. The session bus is
//! pointed at a nonexistent socket so no real daemon can be reached.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(t: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ferroseek"))
        .args(args)
        .env("FERROSEEK_CONFIG", t.join("config.toml"))
        .env("FERROSEEK_DATA_DIR", t.join("data"))
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/no-bus", t.display()))
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn index_search_status_config() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("files");
    fs::create_dir_all(root.join("Music/Album")).unwrap();
    fs::write(root.join("Music/Album/01 - Intro Track.flac"), b"x").unwrap();
    fs::write(root.join("Music/playlist-track-list.m3u"), b"x").unwrap();
    fs::write(root.join(".hidden-track"), b"x").unwrap();
    fs::write(t.path().join("config.toml"), format!("roots = [\"{}\"]\nindex_hidden = true\n", root.display()))
        .unwrap();

    let o = run(t.path(), &["index"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("indexed 3 files"), "{}", stdout(&o));

    let o = run(t.path(), &["search", "track"]);
    assert!(o.status.success());
    let lines: Vec<_> = stdout(&o).lines().map(String::from).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines.iter().any(|l| l.ends_with("01 - Intro Track.flac")), "{lines:?}");
    assert!(lines[2].ends_with(".hidden-track"), "dotfiles rank last: {lines:?}");

    let o = run(t.path(), &["search", "--no-hidden", "track"]);
    assert_eq!(stdout(&o).lines().count(), 2);

    let o = run(t.path(), &["search", "--dirs", "album"]);
    assert!(stdout(&o).trim_end().ends_with("Music/Album"));

    let o = run(t.path(), &["search", "-0", "track", "list"]);
    assert!(o.stdout.ends_with(b"playlist-track-list.m3u\0"));

    let o = run(t.path(), &["search", "--long", "intro"]);
    assert!(stdout(&o).starts_with("f "), "{}", stdout(&o));

    let o = run(t.path(), &["search", "--in", root.join("Music/Album").to_str().unwrap(), "track"]);
    assert_eq!(stdout(&o).lines().count(), 1);

    let o = run(t.path(), &["search", "nothing-matches-this"]);
    assert_eq!(o.status.code(), Some(1), "grep-like exit status");

    let o = run(t.path(), &["status"]);
    let s = stdout(&o);
    assert!(s.contains("files: 3") && s.contains("dirs: 3"), "{s}");

    let o = run(t.path(), &["config"]);
    assert!(stdout(&o).contains("index_hidden = true"));

    let o = run(t.path(), &["rebuild"]);
    assert!(o.status.success() && stdout(&o).contains("indexed 3 files"));
}

#[test]
fn helpful_errors() {
    let t = tempfile::tempdir().unwrap();
    let o = run(t.path(), &["search", "x"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("ferroseek index"));
    fs::write(t.path().join("config.toml"), "bogus = 1\n").unwrap();
    let o = run(t.path(), &["config"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown field"));
    let o = run(t.path(), &["pause"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn json_search_preserves_paths_metadata_order_and_filters() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("files");
    fs::create_dir_all(root.join("report-folder")).unwrap();
    let special = root.join("report-folder/report \"caffè\"\n$(touch nope).txt");
    fs::write(&special, b"hello").unwrap();
    fs::write(root.join("report.txt"), b"abc").unwrap();
    fs::write(root.join(".report-hidden"), b"x").unwrap();
    fs::write(t.path().join("config.toml"), format!("roots = [\"{}\"]\nindex_hidden = true\n", root.display()))
        .unwrap();
    assert!(run(t.path(), &["index"]).status.success());

    let output = run(t.path(), &["search", "--json", "--no-hidden", "report"]);
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["version"], 1);
    let hits = response["results"].as_array().unwrap();
    assert_eq!(hits.len(), 3);
    let raw = run(t.path(), &["search", "-0", "--no-hidden", "report"]);
    let raw_paths: Vec<_> = raw.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
    for (hit, path) in hits.iter().zip(raw_paths) {
        assert_eq!(hit["path"].as_str().unwrap().as_bytes(), path);
        assert!(hit["score"].is_number());
        assert!(hit["mtime_ns"].as_i64().unwrap() > 0);
        assert!(hit.get("path_bytes").is_none());
    }
    let file = hits.iter().find(|h| h["path"].as_str() == special.to_str()).unwrap();
    assert_eq!(file["name"], special.file_name().unwrap().to_str().unwrap());
    assert_eq!(file["size"], 5);
    assert_eq!(file["kind"], "file");

    let scoped =
        run(t.path(), &["search", "--json", "--in", root.join("report-folder").to_str().unwrap(), "-n", "1", "report"]);
    let response: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    assert_eq!(response["results"].as_array().unwrap().len(), 1);
    assert_eq!(response["results"][0]["path"], special.to_str().unwrap());
    let dirs = run(t.path(), &["search", "--json", "--dirs", "report"]);
    let response: serde_json::Value = serde_json::from_slice(&dirs.stdout).unwrap();
    assert_eq!(response["results"].as_array().unwrap().len(), 1);
    assert_eq!(response["results"][0]["kind"], "directory");

    let empty = run(t.path(), &["search", "--json", "no-such-name"]);
    assert_eq!(empty.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&empty.stdout).unwrap(),
        serde_json::json!({
            "version": 1, "results": []
        })
    );
    for flag in ["--long", "-0"] {
        assert_eq!(run(t.path(), &["search", "--json", flag, "report"]).status.code(), Some(2));
    }
    let missing = tempfile::tempdir().unwrap();
    let error = run(missing.path(), &["search", "--json", "report"]);
    assert_eq!(error.status.code(), Some(2));
    assert!(error.stdout.is_empty());
}

#[test]
fn json_non_utf8_paths_are_lossless_and_not_openable_as_display_text() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("files");
    fs::create_dir(&root).unwrap();
    let path = root.join(std::ffi::OsString::from_vec(b"report-\xff.txt".to_vec()));
    fs::write(&path, b"x").unwrap();
    fs::write(t.path().join("config.toml"), format!("roots = [\"{}\"]\n", root.display())).unwrap();
    assert!(run(t.path(), &["index"]).status.success());
    let output = run(t.path(), &["search", "--json", "report"]);
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hit = &response["results"][0];
    assert!(hit["path"].is_null());
    let bytes: Vec<u8> = serde_json::from_value(hit["path_bytes"].clone()).unwrap();
    assert_eq!(bytes, path.as_os_str().as_bytes());
    let raw = run(t.path(), &["search", "-0", "report"]);
    assert_eq!(raw.stdout, [path.as_os_str().as_bytes(), b"\0"].concat());
}

#[test]
fn special_cache_tags_do_not_block_indexing() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    for connected_writer in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("report.txt"), b"hello").unwrap();
        let tag = root.join("CACHEDIR.TAG");
        let c_path = std::ffi::CString::new(tag.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a valid NUL-terminated path in this test's temp directory.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let _writer = connected_writer
            .then(|| fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(&tag).unwrap());
        fs::write(t.path().join("config.toml"), format!("roots = [\"{}\"]\n", root.display())).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ferroseek"))
            .arg("index")
            .env("FERROSEEK_CONFIG", t.path().join("config.toml"))
            .env("FERROSEEK_DATA_DIR", t.path().join("data"))
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/no-bus", t.path().display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("indexing blocked on CACHEDIR.TAG FIFO; connected_writer={connected_writer}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(stdout(&run(t.path(), &["search", "report"])).contains("report.txt"));
    }
}
