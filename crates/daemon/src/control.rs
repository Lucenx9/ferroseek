// SPDX-License-Identifier: GPL-3.0-or-later
//! The daemon's own control interface, used by the `ferroseek` CLI.

use std::collections::HashMap;
use std::sync::Arc;

use ferroseek_engine::{Engine, Paths};

pub struct Control {
    pub engine: Arc<Engine>,
    pub paths: Paths,
}

#[zbus::interface(name = "io.github.ferroseek.FerroSeek1")]
impl Control {
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
