// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit favorites, stored separately from decaying launch history.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::uri;
use crate::usage::{self, Usage};

const MAX_FAVORITES: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    App,
    File,
    Command,
    Link,
    Snippet,
}

impl From<usage::Kind> for Kind {
    fn from(kind: usage::Kind) -> Self {
        match kind {
            usage::Kind::App => Self::App,
            usage::Kind::File => Self::File,
        }
    }
}

impl Kind {
    fn parse(tag: &str) -> Option<Self> {
        match tag {
            "app" => Some(Self::App),
            "file" => Some(Self::File),
            "command" => Some(Self::Command),
            "link" => Some(Self::Link),
            "snippet" => Some(Self::Snippet),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::File => "file",
            Self::Command => "command",
            Self::Link => "link",
            Self::Snippet => "snippet",
        }
    }
}

#[derive(Debug, Default)]
pub struct Favorites {
    path: Option<PathBuf>,
    items: Vec<(Kind, String)>,
}

impl Favorites {
    pub fn default_path() -> Option<PathBuf> {
        Usage::default_path().map(|path| path.with_file_name("launcher-favorites"))
    }

    pub fn load(path: Option<PathBuf>) -> Self {
        let items = path.as_deref().and_then(|path| read_items(path).ok()).unwrap_or_default();
        Self { path, items }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn is_persistent(&self) -> bool {
        self.path.is_some()
    }

    pub fn contains(&self, kind: Kind, key: &str) -> bool {
        self.items.iter().any(|(stored_kind, stored_key)| *stored_kind == kind && stored_key == key)
    }

    pub fn items(&self) -> &[(Kind, String)] {
        &self.items
    }

    /// Applies the requested state to the latest stored list. Contention and
    /// I/O errors leave the session's cache unchanged so callers can retry.
    pub fn set(&mut self, kind: Kind, key: &str, enabled: bool) -> io::Result<()> {
        if !valid_key(kind, key) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid favorite key"));
        }
        self.edit(|items| update_items(items, kind, key, enabled))
    }

    /// Swaps two visible favorites, leaving hidden entries in their slots.
    pub fn swap_in_order(&mut self, earlier: (Kind, &str), later: (Kind, &str)) -> io::Result<()> {
        self.edit(|items| {
            let index = |(kind, key)| items.iter().position(|(k, value)| (*k, value.as_str()) == (kind, key));
            match (index(earlier), index(later)) {
                (Some(first), Some(second)) if first < second => {
                    items.swap(first, second);
                    Ok(true)
                }
                _ => Err(io::Error::other("The order changed on disk. Reopen FerroSeek to load it.")),
            }
        })
    }

    fn edit(&mut self, change: impl FnOnce(&mut Vec<(Kind, String)>) -> io::Result<bool>) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return change(&mut self.items).map(|_| ());
        };
        let items = usage::with_lock(&path, false, || {
            let mut items = read_items(&path)?;
            if change(&mut items)? {
                write_items(&path, &items)?;
            }
            Ok(items)
        })?;
        self.items = items;
        Ok(())
    }
}

fn valid_key(kind: Kind, key: &str) -> bool {
    !key.is_empty()
        && !key.contains(['\n', '\r', '\t'])
        && (kind != Kind::File || uri::path_from_file_url(key).is_some())
}

fn update_items(items: &mut Vec<(Kind, String)>, kind: Kind, key: &str, enabled: bool) -> io::Result<bool> {
    match (items.iter().position(|(k, value)| *k == kind && value == key), enabled) {
        (Some(index), false) => {
            items.remove(index);
            Ok(true)
        }
        (None, true) => {
            if items.len() == MAX_FAVORITES {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "favorite limit reached"));
            }
            items.push((kind, key.to_string()));
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn read_items(path: &Path) -> io::Result<Vec<(Kind, String)>> {
    let bytes = match usage::read_state_file(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut items = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if items.len() == MAX_FAVORITES {
            break;
        }
        let Ok(line) = std::str::from_utf8(line) else { continue };
        let Some((tag, key)) = line.split_once('\t') else { continue };
        let Some(kind) = Kind::parse(tag) else { continue };
        if valid_key(kind, key) && !items.iter().any(|(k, value)| *k == kind && value == key) {
            items.push((kind, key.to_string()));
        }
    }
    Ok(items)
}

fn write_items(path: &Path, items: &[(Kind, String)]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut file = usage::private_options().write(true).create_new(true).open(&tmp)?;
        for (kind, key) in items {
            let tag = kind.tag();
            writeln!(file, "{tag}\t{key}")?;
        }
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_persist_in_added_order_with_private_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/launcher-favorites");
        let mut favorites = Favorites::load(Some(path.clone()));
        assert!(favorites.is_persistent());
        favorites.set(Kind::App, "a.desktop", true).unwrap();
        favorites.set(Kind::File, "file:///synthetic/a%20b%FF", true).unwrap();
        favorites.set(Kind::App, "b.desktop", true).unwrap();
        favorites.set(Kind::Command, "switch-windows", true).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        favorites.set(Kind::App, "a.desktop", true).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode, "an idempotent request does not replace the file");
        favorites.set(Kind::App, "a.desktop", false).unwrap();
        favorites.set(Kind::App, "a.desktop", true).unwrap();
        assert_eq!(
            favorites.items(),
            [
                (Kind::File, "file:///synthetic/a%20b%FF".to_string()),
                (Kind::App, "b.desktop".to_string()),
                (Kind::Command, "switch-windows".to_string()),
                (Kind::App, "a.desktop".to_string()),
            ]
        );
        assert_eq!(Favorites::load(Some(path.clone())).items(), favorites.items());
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&path.with_extension("lock")), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[test]
    fn stale_instances_merge_pin_and_unpin_requests_without_resurrecting_items() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut a = Favorites::load(Some(path.clone()));
        let mut b = Favorites::load(Some(path.clone()));
        a.set(Kind::App, "a.desktop", true).unwrap();
        b.set(Kind::App, "b.desktop", true).unwrap();
        a.set(Kind::App, "a.desktop", false).unwrap();
        b.set(Kind::App, "b.desktop", true).unwrap();
        assert_eq!(b.items(), [(Kind::App, "b.desktop".to_string())]);
        a.set(Kind::App, "a.desktop", true).unwrap();
        assert_eq!(
            Favorites::load(Some(path)).items(),
            [(Kind::App, "b.desktop".to_string()), (Kind::App, "a.desktop".to_string())]
        );
    }

    #[test]
    fn reordering_keeps_hidden_slots_and_rejects_a_stale_direction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut favorites = Favorites::load(Some(path.clone()));
        let app = (Kind::App, "a.desktop");
        let file = (Kind::File, "file:///synthetic/file.txt");
        let command = (Kind::Command, "switch-windows");
        for (kind, key) in [app, file, command] {
            favorites.set(kind, key, true).unwrap();
        }
        let mut stale = Favorites::load(Some(path.clone()));
        favorites.swap_in_order(app, command).unwrap();
        assert_eq!(favorites.items(), [command, file, app].map(|(kind, key)| (kind, key.to_owned())));
        assert!(stale.swap_in_order(app, command).is_err());
        assert_eq!(stale.items(), [app, file, command].map(|(kind, key)| (kind, key.to_owned())));
        assert_eq!(Favorites::load(Some(path)).items(), favorites.items());
    }

    #[test]
    fn loading_skips_invalid_and_duplicate_rows_before_applying_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut text = "unknown\tignored\napp\t\nfile\thttp://host/x\napp\tbad\tkey\napp\tbad\rcode\n".repeat(500);
        text.push_str("app\tfirst.desktop\napp\tfirst.desktop\nfile\tfile:///synthetic/a%FF\n");
        for i in 0..MAX_FAVORITES {
            text.push_str(&format!("app\tapp{i}.desktop\n"));
        }
        let bytes = [b"app\tinvalid-\xff\n".as_slice(), text.as_bytes()].concat();
        std::fs::write(&path, &bytes).unwrap();
        let mut favorites = Favorites::load(Some(path.clone()));
        assert_eq!(favorites.items().len(), MAX_FAVORITES);
        assert_eq!(favorites.items()[0], (Kind::App, "first.desktop".to_string()));
        assert_eq!(favorites.items()[1], (Kind::File, "file:///synthetic/a%FF".to_string()));
        assert_eq!(favorites.items().last().unwrap().1, format!("app{}.desktop", MAX_FAVORITES - 3));
        favorites.set(Kind::App, "first.desktop", true).unwrap();
        let before = favorites.items().to_vec();
        assert_eq!(
            favorites.set(Kind::App, "over-limit.desktop", true).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(favorites.items(), before);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        favorites.set(Kind::App, "first.desktop", false).unwrap();
        favorites.set(Kind::App, "replacement.desktop", true).unwrap();
        assert_eq!(favorites.items().len(), MAX_FAVORITES);
        assert_eq!(favorites.items().last().unwrap().1, "replacement.desktop");
    }

    #[test]
    fn invalid_keys_are_rejected_without_creating_storage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/launcher-favorites");
        let mut favorites = Favorites::load(Some(path.clone()));
        for (kind, key) in [
            (Kind::App, ""),
            (Kind::App, "a\tb"),
            (Kind::App, "a\nb"),
            (Kind::App, "a\rb"),
            (Kind::File, "http:///x"),
            (Kind::File, "file://host/x"),
            (Kind::File, "file:///x%2"),
            (Kind::File, "file:///x%00"),
        ] {
            assert_eq!(favorites.set(kind, key, true).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
        assert!(!path.parent().unwrap().exists());
        favorites.set(Kind::App, "absent.desktop", false).unwrap();
        assert!(favorites.items().is_empty());
        assert!(!path.exists(), "an absent favorite is not written");
    }

    #[test]
    fn failed_reads_and_writes_preserve_the_cached_list_and_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut favorites = Favorites::load(Some(path.clone()));
        favorites.set(Kind::App, "saved.desktop", true).unwrap();
        let before = favorites.items().to_vec();
        let disk = std::fs::read(&path).unwrap();
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::create_dir(&tmp).unwrap();
        assert!(favorites.set(Kind::App, "new.desktop", true).is_err());
        assert_eq!(favorites.items(), before);
        assert_eq!(std::fs::read(&path).unwrap(), disk);

        std::fs::remove_dir(tmp).unwrap();
        let saved = dir.path().join("saved");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(favorites.set(Kind::App, "new.desktop", true).is_err());
        assert_eq!(favorites.items(), before);
        assert_eq!(std::fs::read(saved).unwrap(), disk);
    }

    #[test]
    fn a_busy_lock_returns_without_waiting_or_changing_state() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut favorites = Favorites::load(Some(path.clone()));
        favorites.set(Kind::App, "saved.desktop", true).unwrap();
        let before = favorites.items().to_vec();
        let disk = std::fs::read(&path).unwrap();
        let lock = usage::private_options().read(true).write(true).open(path.with_extension("lock")).unwrap();
        lock.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = favorites.set(Kind::App, "new.desktop", true);
            tx.send((favorites, result)).unwrap();
        });
        let received = rx.recv_timeout(Duration::from_secs(2));
        lock.unlock().unwrap();
        worker.join().unwrap();
        let (mut favorites, result) = received.expect("favorites must not wait for a busy history lock");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert_eq!(favorites.items(), before);
        assert_eq!(std::fs::read(&path).unwrap(), disk);
        favorites.set(Kind::App, "new.desktop", true).unwrap();
        assert!(Favorites::load(Some(path)).contains(Kind::App, "new.desktop"));
    }

    #[test]
    fn private_favorites_stay_in_memory_and_keep_kinds_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut persistent = Favorites::load(Some(path.clone()));
        persistent.set(Kind::App, "saved.desktop", true).unwrap();
        let disk = std::fs::read(&path).unwrap();
        let mut favorites = Favorites::disabled();
        assert!(!favorites.is_persistent());
        assert!(!favorites.contains(Kind::App, "saved.desktop"));
        let key = "file:///synthetic/not-present%FF";
        favorites.set(Kind::App, key, true).unwrap();
        favorites.set(Kind::File, key, true).unwrap();
        favorites.set(Kind::App, key, false).unwrap();
        assert!(favorites.contains(Kind::File, key));
        assert!(!favorites.contains(Kind::App, key));
        assert!(Favorites::disabled().items().is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), disk);
    }

    #[test]
    fn favorites_are_independent_of_usage_reset_and_forget() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("launcher-usage");
        let path = history.with_file_name("launcher-favorites");
        let mut usage = Usage::load(Some(history.clone()));
        let mut favorites = Favorites::load(Some(path.clone()));
        usage.record(usage::Kind::App, "saved.desktop", "sav", 100);
        favorites.set(Kind::App, "saved.desktop", true).unwrap();
        usage.reset(usage::Kind::App, "saved.desktop").unwrap();
        assert!(Favorites::load(Some(path.clone())).contains(Kind::App, "saved.desktop"));
        usage.record(usage::Kind::App, "saved.desktop", "sav", 100);
        let disk = std::fs::read(&history).unwrap();
        favorites.set(Kind::App, "saved.desktop", false).unwrap();
        assert_eq!(std::fs::read(&history).unwrap(), disk);
        favorites.set(Kind::App, "saved.desktop", true).unwrap();
        Usage::forget(&history).unwrap();
        assert!(Favorites::load(Some(path)).contains(Kind::App, "saved.desktop"));
        assert!(!history.exists());
    }
}
