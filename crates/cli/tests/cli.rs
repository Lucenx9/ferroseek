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
