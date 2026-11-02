// SPDX-License-Identifier: GPL-3.0-or-later
//! File search and opening for the standalone launcher.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use ferroseek_index::{Hit, Kind, Query, SearchOptions};

use crate::mime::MimeDb;
use crate::open;
use crate::pool::Pool;

const SEARCH_LIMIT: usize = 15;
const ACTION_OPEN_PARENT: &str = "open-parent";

/// URL, name, icon, parent display path and directory flag.
pub type FileResult = (String, String, String, String, bool);

pub struct Search {
    pool: Arc<Pool>,
    mime: Arc<OnceLock<MimeDb>>,
    home: PathBuf,
}

impl Search {
    pub fn new(pool: Arc<Pool>) -> Search {
        Search {
            pool,
            mime: Arc::new(OnceLock::new()),
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        }
    }
    async fn indexed_file(&self, match_id: &str) -> zbus::fdo::Result<PathBuf> {
        let path = path_from_url(match_id).ok_or_else(|| zbus::fdo::Error::InvalidArgs("invalid file URL".into()))?;
        let pool = self.pool.clone();
        let p = path.clone();
        zruntime::unblock(move || {
            let known = pool.with(|r| r.lookup(&p)).map_err(|_| {
                zbus::fdo::Error::AccessDenied("Cannot authorize file while index is unavailable".into())
            })?;
            if !known.is_some_and(|(_, meta)| meta.kind == Kind::File) {
                return Err(zbus::fdo::Error::AccessDenied("path is not a regular indexed file".into()));
            }
            if !std::fs::symlink_metadata(&p).is_ok_and(|meta| meta.is_file()) {
                return Err(zbus::fdo::Error::AccessDenied("file is no longer available".into()));
            }
            Ok(())
        })
        .await?;
        Ok(path)
    }
}

fn applications_for_file(path: &Path) -> Vec<ferroseek_desktop::apps::App> {
    let db = MimeDb::load_for_handlers();
    let types = db.handler_types(&path.file_name().unwrap_or_default().to_string_lossy());
    if types.is_empty() {
        return Vec::new();
    }
    ferroseek_desktop::apps::file_handlers(
        &ferroseek_desktop::apps::data_dirs(),
        &ferroseek_desktop::apps::current_locales(),
        &ferroseek_desktop::apps::current_desktops(),
        &types,
    )
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

fn to_result(h: &Hit, mime: &MimeDb, home: &Path) -> FileResult {
    let name = h.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let icon = match h.kind {
        Kind::Dir => "folder".to_string(),
        Kind::Symlink if mime.mime_for(&name).is_none() => "inode-symlink".to_string(),
        _ => mime.icon_for(&name),
    };
    (file_url(&h.path), name, icon, subtext(&h.path, home), h.kind == Kind::Dir)
}

#[zbus::interface(name = "io.github.ferroseek.Search1")]
impl Search {
    /// Explicit bounded text preview; no filesystem path is accepted as a command.
    async fn text_preview(&self, match_id: String) -> zbus::fdo::Result<(String, bool)> {
        let path = path_from_url(&match_id).ok_or_else(|| zbus::fdo::Error::InvalidArgs("invalid file URL".into()))?;
        let pool = self.pool.clone();
        zruntime::unblock(move || {
            let meta = pool
                .with(|reader| reader.lookup(&path))
                .map_err(|_| zbus::fdo::Error::AccessDenied("Cannot authorize file while index is unavailable".into()))?
                .filter(|(_, meta)| meta.kind == Kind::File)
                .map(|(_, meta)| meta)
                .ok_or_else(|| zbus::fdo::Error::AccessDenied("Path is not a regular indexed file".into()))?;
            let db = MimeDb::load();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let mime = db.mime_for(&name).unwrap_or_default();
            crate::preview::read(&path, meta.ino, mime).map_err(|error| match error {
                crate::preview::Error::Unsupported(reason) => zbus::fdo::Error::NotSupported(reason.into()),
                crate::preview::Error::Unavailable => zbus::fdo::Error::AccessDenied(
                    "File changed or is unavailable. Refresh after indexing catches up.".into(),
                ),
                crate::preview::Error::Io(error) => zbus::fdo::Error::Failed(format!("Cannot read preview: {error}")),
            })
        })
        .await
    }

    /// Search strictly below a currently indexed directory. The path is kept
    /// separate from user query syntax, including spaces and non-UTF-8 bytes.
    async fn search_within(&self, folder_id: String, query: String) -> zbus::fdo::Result<Vec<FileResult>> {
        let path =
            path_from_url(&folder_id).ok_or_else(|| zbus::fdo::Error::InvalidArgs("invalid folder URL".into()))?;
        let (pool, mime, home) = (self.pool.clone(), self.mime.clone(), self.home.clone());
        zruntime::unblock(move || {
            let hits = pool
                .with(|reader| {
                    let indexed = reader.lookup(&path)?.is_some_and(|(_, meta)| meta.kind == Kind::Dir);
                    if !indexed || !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                        return Ok(None);
                    }
                    if query.trim().chars().count() < 2 {
                        return Ok(Some(Vec::new()));
                    }
                    reader
                        .search(
                            &Query::parse(&query),
                            &SearchOptions { limit: SEARCH_LIMIT, within: Some(path.clone()), ..Default::default() },
                        )
                        .map(Some)
                })
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            let hits = hits.ok_or_else(|| {
                zbus::fdo::Error::AccessDenied("Folder is no longer an available indexed directory".into())
            })?;
            let mime = mime.get_or_init(MimeDb::load);
            Ok(hits.iter().map(|hit| to_result(hit, mime, &home)).collect())
        })
        .await
    }

    async fn search(&self, query: String) -> zbus::fdo::Result<Vec<FileResult>> {
        if query.trim().chars().count() < 2 {
            return Ok(Vec::new());
        }
        let (pool, mime, home) = (self.pool.clone(), self.mime.clone(), self.home.clone());
        zruntime::unblock(move || {
            let opts = SearchOptions { limit: SEARCH_LIMIT, ..Default::default() };
            let hits = pool
                .with(|r| r.search(&Query::parse(&query), &opts))
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            let mime = mime.get_or_init(MimeDb::load);
            Ok(hits.iter().map(|h| to_result(h, mime, &home)).collect())
        })
        .await
    }

    /// Current attributes for an indexed regular file, without reading its contents.
    async fn file_details(&self, match_id: String) -> zbus::fdo::Result<(String, u64, i64)> {
        use std::os::unix::fs::MetadataExt;

        let path = self.indexed_file(&match_id).await?;
        zruntime::unblock(move || {
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|_| zbus::fdo::Error::AccessDenied("File is no longer available".into()))?;
            if !meta.is_file() {
                return Err(zbus::fdo::Error::AccessDenied("Path is no longer a regular file".into()));
            }
            let mime = MimeDb::load();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            Ok((mime.mime_for(&name).unwrap_or_default().to_owned(), meta.len(), meta.mtime()))
        })
        .await
    }

    /// Installed handlers declaring the indexed file's MIME type or a parent.
    async fn applications(&self, match_id: String) -> zbus::fdo::Result<Vec<(String, String, String)>> {
        let path = self.indexed_file(&match_id).await?;
        zruntime::unblock(move || {
            Ok(applications_for_file(&path)
                .into_iter()
                .map(|app| (app.id, app.name, app.icon.unwrap_or_default()))
                .collect())
        })
        .await
    }

    /// Open one indexed file with a current installed handler, without
    /// modifying the desktop's default application association.
    async fn open_with(&self, match_id: String, application_id: String, token: String) -> zbus::fdo::Result<()> {
        let path = self.indexed_file(&match_id).await?;
        zruntime::unblock(move || {
            let app =
                applications_for_file(&path).into_iter().find(|app| app.id == application_id).ok_or_else(|| {
                    zbus::fdo::Error::InvalidArgs(
                        "Application is no longer available for this file. Reload the application list.".into(),
                    )
                })?;
            let uri = file_url(&path);
            let mut child = ferroseek_desktop::launch::launch_file(
                &app,
                &path,
                &uri,
                (!token.is_empty()).then_some(token.as_str()),
            )
            // A preparation/spawn error returns before a child is handed back.
            // The selected application has not executed; an explicit retry is safe.
            .map_err(|e| zbus::fdo::Error::SpawnExecFailed(e.to_string()))?;
            // From here the child may already have opened the document, so a
            // reaper failure must remain an unconfirmed dispatch to the client.
            std::thread::Builder::new()
                .name("reap-selected-opener".into())
                .spawn(move || {
                    let _ = child.wait();
                })
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn open(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        match_id: String,
        action_id: String,
        token: String,
    ) -> zbus::fdo::Result<()> {
        let token = if token.is_empty() { None } else { Some(token.as_str()) };
        let Some(path) = path_from_url(&match_id) else {
            return Err(zbus::fdo::Error::InvalidArgs("invalid file URL".into()));
        };
        // Only open what we indexed: Open can be called by any session-bus
        // client, and this daemon must not become a generic launcher.
        let pool = self.pool.clone();
        let p = path.clone();
        let known = zruntime::unblock(move || pool.with(|r| r.lookup(&p)).ok().flatten().is_some()).await;
        if !known {
            return Err(zbus::fdo::Error::AccessDenied("path is not in the index".into()));
        }
        log::debug!("Open {} action {action_id:?}", path.display());
        let res = match action_id.as_str() {
            "" => open::open_path(&path, token),
            ACTION_OPEN_PARENT => open::show_in_folder(conn, &path, token).await,
            other => {
                return Err(zbus::fdo::Error::InvalidArgs(format!("unknown action {other:?}")));
            }
        };
        res.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn release(&self) {
        // End of a launcher session: release reader connections and their
        // page caches until the next session.
        let pool = self.pool.clone();
        zruntime::unblock(move || {
            pool.clear();
            ferroseek_engine::sys::trim_heap();
        })
        .await;
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
    fn subtext_collapses_home() {
        let h = Path::new("/home/u");
        assert_eq!(subtext(Path::new("/home/u/a.txt"), h), "~");
        assert_eq!(subtext(Path::new("/home/u/d/a.txt"), h), "~/d");
        assert_eq!(subtext(Path::new("/srv/a.txt"), h), "/srv");
    }
}
