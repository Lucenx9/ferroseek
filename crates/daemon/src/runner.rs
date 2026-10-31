// SPDX-License-Identifier: GPL-3.0-or-later
//! KRunner D-Bus runner (`org.kde.krunner1`).
//!
//! Contract verified against KRunner 6.31 sources; see docs/RESEARCH.md §10.
//! Matches are computed on a blocking pool thread so concurrent `Match`
//! calls (one per keystroke, never cancelled by KRunner) do not serialize,
//! and a generation counter drops superseded queries early.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferroseek_index::{Hit, Kind, Query, SearchOptions};
use zbus::{OwnedVariant as OwnedValue, Variant as Value};

use crate::mime::MimeDb;
use crate::open;
use crate::pool::Pool;

/// `KRunner::QueryMatch::CategoryRelevance` (krunner/src/querymatch.h).
pub const CATEGORY_HIGHEST: i32 = 100;
pub const CATEGORY_HIGH: i32 = 70;
pub const CATEGORY_LOW: i32 = 30;

/// Results per `Match`; KRunner shows only a handful per runner.
const MATCH_LIMIT: usize = 15;
const ACTION_OPEN_PARENT: &str = "open-parent";

/// One element of `a(sssida{sv})`.
pub type RemoteMatch = (String, String, String, i32, f64, HashMap<String, OwnedValue>);

pub struct Runner {
    pool: Arc<Pool>,
    mime: Arc<OnceLock<MimeDb>>,
    generation: Arc<AtomicU64>,
    token: Mutex<Option<String>>,
    home: PathBuf,
}

impl Runner {
    pub fn new(pool: Arc<Pool>) -> Runner {
        Runner {
            pool,
            mime: Arc::new(OnceLock::new()),
            generation: Arc::new(AtomicU64::new(0)),
            token: Mutex::new(None),
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        }
    }
}

/// `file://` URL with every byte outside RFC 3986 unreserved/`/` escaped,
/// so any path (including non-UTF-8 bytes) becomes a valid D-Bus string.
pub fn file_url(p: &Path) -> String {
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

pub fn path_from_url(u: &str) -> Option<PathBuf> {
    let rest = u.strip_prefix("file://")?;
    if !rest.starts_with('/') {
        return None; // file://host/... is not ours
    }
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(&out)))
}

/// Parent directory for display, with `$HOME` shown as `~`.
fn subtext(path: &Path, home: &Path) -> String {
    let parent = path.parent().unwrap_or(Path::new("/"));
    match parent.strip_prefix(home) {
        Ok(rel) if !home.as_os_str().is_empty() => {
            if rel.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rel.to_string_lossy())
            }
        }
        _ => parent.to_string_lossy().into_owned(),
    }
}

fn ov(v: Value<'_>) -> OwnedValue {
    // Only plain strings/arrays here; conversion cannot fail (no fds).
    v.try_into().expect("string values convert to OwnedValue")
}

/// Mirrors Baloo's runner: Highest for an exact name/stem match, High when
/// the whole query is a substring of the name, Low otherwise.
pub fn category_relevance(name: &str, query: &str) -> i32 {
    let n = name.to_lowercase();
    let q = query.trim().to_lowercase();
    let stem = match n.rfind('.') {
        Some(p) if p > 0 => &n[..p],
        _ => n.as_str(),
    };
    if n == q || stem == q {
        CATEGORY_HIGHEST
    } else if n.contains(&q) {
        CATEGORY_HIGH
    } else {
        CATEGORY_LOW
    }
}

pub fn to_match(h: &Hit, query: &str, mime: &MimeDb, home: &Path) -> RemoteMatch {
    let name = h.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let url = file_url(&h.path);
    let (icon, category) = match h.kind {
        Kind::Dir => ("folder".to_string(), "Folders"),
        Kind::Symlink if mime.mime_for(&name).is_none() => ("inode-symlink".to_string(), "Files"),
        _ => (mime.icon_for(&name), mime.category_for(&name)),
    };
    let mut props = HashMap::new();
    props.insert("urls".to_string(), ov(Value::from(vec![url.clone()])));
    props.insert("subtext".to_string(), ov(Value::from(subtext(&h.path, home))));
    props.insert("category".to_string(), ov(Value::from(category)));
    props.insert("actions".to_string(), ov(Value::from(vec![ACTION_OPEN_PARENT.to_string()])));
    (url, name.clone(), icon, category_relevance(&name, query), h.relevance(), props)
}

#[zbus::interface(name = "org.kde.krunner1")]
impl Runner {
    async fn actions(&self) -> Vec<(String, String, String)> {
        vec![(ACTION_OPEN_PARENT.into(), "Open Containing Folder".into(), "document-open-folder".into())]
    }

    #[zbus(name = "Match")]
    async fn do_match(&self, query: String) -> Vec<RemoteMatch> {
        let my_gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if query.trim().chars().count() < 2 {
            return Vec::new();
        }
        let (pool, mime, latest, home) = (self.pool.clone(), self.mime.clone(), self.generation.clone(), self.home.clone());
        zruntime::unblock(move || {
            // KRunner drops replies for superseded queries; skip the work.
            if latest.load(Ordering::SeqCst) != my_gen {
                return Vec::new();
            }
            let opts = SearchOptions { limit: MATCH_LIMIT, ..Default::default() };
            let hits = match pool.with(|r| r.search(&Query::parse(&query), &opts)) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("KRunner query failed: {e}");
                    return Vec::new();
                }
            };
            if latest.load(Ordering::SeqCst) != my_gen {
                return Vec::new();
            }
            let mime = mime.get_or_init(MimeDb::load);
            log::debug!("Match {query:?}: {} results", hits.len());
            hits.iter().map(|h| to_match(h, &query, mime, &home)).collect()
        })
        .await
    }

    async fn set_activation_token(&self, token: String) {
        *self.token.lock().unwrap_or_else(|e| e.into_inner()) = Some(token);
    }

    async fn run(&self, #[zbus(connection)] conn: &zbus::Connection, match_id: String, action_id: String) {
        let token = self.token.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(path) = path_from_url(&match_id) else {
            log::warn!("Run: ignoring non-file match id");
            return;
        };
        // Only open what we indexed: Run can be called by any session-bus
        // client, and this daemon must not become a generic launcher.
        let pool = self.pool.clone();
        let p = path.clone();
        let known = zruntime::unblock(move || pool.with(|r| r.lookup(&p)).ok().flatten().is_some()).await;
        if !known {
            log::warn!("Run: {} is not in the index; ignoring", path.display());
            return;
        }
        log::debug!("Run {} action {action_id:?}", path.display());
        let res = match action_id.as_str() {
            "" => open::open_path(&path, token.as_deref()),
            ACTION_OPEN_PARENT => open::show_in_folder(conn, &path, token.as_deref()).await,
            other => {
                log::warn!("Run: unknown action {other:?}");
                Ok(())
            }
        };
        if let Err(e) = res {
            log::warn!("Run: opening {} failed: {e}", path.display());
        }
    }

    async fn teardown(&self) {
        // End of a KRunner session: release reader connections and their
        // page caches until the next session.
        self.pool.clear();
        ferroseek_engine::sys::trim_heap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_round_trip_including_non_utf8() {
        for p in ["/home/u/My Docs/a#b?c%.txt", "/tmp/ünï/日本", "/x/y z/+&=;"] {
            let u = file_url(Path::new(p));
            assert!(u.is_ascii());
            assert_eq!(path_from_url(&u).unwrap(), Path::new(p));
        }
        let raw = PathBuf::from(OsStr::from_bytes(b"/t/bad\xff\xfe"));
        assert_eq!(path_from_url(&file_url(&raw)).unwrap(), raw);
        assert_eq!(file_url(Path::new("/a b")), "file:///a%20b");
        assert!(path_from_url("file://host/x").is_none());
        assert!(path_from_url("https://x/y").is_none());
        assert!(path_from_url("file:///a%00b").is_none());
        assert!(path_from_url("file:///a%zz").is_none());
    }

    #[test]
    fn category_relevance_classes() {
        assert_eq!(category_relevance("Report.pdf", "report"), CATEGORY_HIGHEST);
        assert_eq!(category_relevance("report.pdf", "report.pdf"), CATEGORY_HIGHEST);
        assert_eq!(category_relevance("annual-report.pdf", "report"), CATEGORY_HIGH);
        assert_eq!(category_relevance("annual-report.pdf", "annual pdf"), CATEGORY_LOW);
    }

    #[test]
    fn subtext_collapses_home() {
        let h = Path::new("/home/u");
        assert_eq!(subtext(Path::new("/home/u/a.txt"), h), "~");
        assert_eq!(subtext(Path::new("/home/u/d/a.txt"), h), "~/d");
        assert_eq!(subtext(Path::new("/srv/a.txt"), h), "/srv");
    }
}
