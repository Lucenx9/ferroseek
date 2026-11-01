// SPDX-License-Identifier: GPL-3.0-or-later
//! The daemon's own control interface, used by the `ferroseek` CLI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use ferroseek_engine::{Engine, Paths, Snapshot};

type FileSearchStatus = (String, u64, u64, u64, bool, u64, Vec<String>);

pub struct Control {
    pub engine: Arc<Engine>,
    pub paths: Paths,
}

#[zbus::interface(name = "io.github.ferroseek.FerroSeek1")]
impl Control {
    /// Typed status for the launcher, including the indexed roots as an array.
    async fn file_search_status(&self) -> zbus::fdo::Result<(String, u64, u64, u64, bool, u64, Vec<String>)> {
        let snapshot = self.engine.status();
        let db = self.paths.index_file();
        zruntime::unblock(move || read_file_search_status(&db, snapshot)).await
    }

    /// Engine state and live index counts as string pairs.
    async fn status(&self) -> HashMap<String, String> {
        let mut m: HashMap<String, String> =
            self.engine.status().pairs().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        // A short-lived reader: status calls must not leave a cached
        // connection behind (the pool is emptied only by launcher Release).
        let db = self.paths.index_file();
        if let Ok(s) = zruntime::unblock(move || ferroseek_index::Reader::open(&db).and_then(|r| r.stats())).await {
            m.insert("files".into(), s.files.to_string());
            m.insert("dirs".into(), (s.dirs).to_string());
            m.insert("disk_bytes".into(), s.disk_bytes.to_string());
            m.insert(
                "roots".into(),
                s.roots.iter().map(|r| r.to_string_lossy().into_owned()).collect::<Vec<_>>().join(":"),
            );
        }
        m.insert("index_path".into(), self.paths.index_file().to_string_lossy().into_owned());
        m.insert("version".into(), env!("CARGO_PKG_VERSION").into());
        m.insert("pid".into(), std::process::id().to_string());
        m
    }

    async fn pause(&self) {
        let engine = self.engine.clone();
        zruntime::unblock(move || engine.pause()).await;
    }

    async fn resume(&self) {
        let engine = self.engine.clone();
        zruntime::unblock(move || engine.resume()).await;
    }

    /// Reconcile all roots; `full` re-stats every file.
    async fn reindex(&self, full: bool) {
        let engine = self.engine.clone();
        zruntime::unblock(move || engine.reindex(full)).await;
    }

    /// Drop the index and rebuild it from scratch.
    async fn rebuild(&self) {
        let engine = self.engine.clone();
        zruntime::unblock(move || engine.rebuild()).await;
    }
}

fn read_file_search_status(db: &Path, snapshot: Snapshot) -> zbus::fdo::Result<FileSearchStatus> {
    let stats = ferroseek_index::Reader::open(db).and_then(|reader| reader.stats()).map_err(|e| {
        log::warn!("cannot read file-search status: {e}");
        zbus::fdo::Error::Failed("File-search status is unavailable. Try again.".into())
    })?;
    Ok((
        snapshot.state,
        stats.files,
        stats.dirs,
        snapshot.queued_dirs,
        snapshot.watch_limit_hit,
        snapshot.errors,
        stats.roots.into_iter().map(|root| root.to_string_lossy().into_owned()).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferroseek_index::{Kind, Meta, Writer};

    #[test]
    fn file_search_status_reads_live_counts_without_mutating_the_index() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("index.sqlite");
        let root = temp.path().join("root:colon");
        let (mut writer, _) = Writer::open(&db).unwrap();
        let mut tx = writer.transaction().unwrap();
        let meta = Meta { kind: Kind::Dir, size: 0, mtime_ns: 0, ino: 1 };
        let id = tx.ensure_root(&root, meta).unwrap();
        tx.upsert(id, b"file.txt", Meta { kind: Kind::File, ..meta }).unwrap();
        tx.commit().unwrap();
        let snapshot = Snapshot {
            state: "paused".into(),
            files: 999,
            dirs: 999,
            queued_dirs: 3,
            watch_limit_hit: true,
            errors: 5,
            ..Default::default()
        };
        let expected = ("paused".into(), 1, 1, 3, true, 5, vec![root.to_string_lossy().into_owned()]);
        assert_eq!(read_file_search_status(&db, snapshot.clone()).unwrap(), expected);
        assert_eq!(read_file_search_status(&db, snapshot).unwrap(), expected);
        assert_eq!(writer.stats().unwrap().files, 1);
        writer.close().unwrap();
    }

    #[test]
    fn unavailable_status_returns_a_friendly_error_without_creating_an_index() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("missing.sqlite");
        let error = read_file_search_status(&db, Snapshot::default()).unwrap_err();
        assert!(error.to_string().contains("File-search status is unavailable. Try again."));
        assert!(!db.exists());
    }
}
