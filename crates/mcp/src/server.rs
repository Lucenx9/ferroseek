// SPDX-License-Identifier: GPL-3.0-or-later
//! The MCP tools. Every response is built from the existing index through a
//! read-only [`Reader`] and filtered by the [`Policy`] before it leaves the
//! process.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferroseek_index::{Hit, Kind, Meta, Query, Reader, SearchOptions};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::beneath::stat_beneath;
use crate::policy::{self, Denied, Policy};

/// Upper bound on the serialized `results` array of one `search_files`
/// response (the SDK also mirrors structured output as text, so the wire
/// size is about twice this).
pub const RESULT_BYTES_BUDGET: usize = 24 * 1024;
const DEFAULT_LIMIT: usize = 20;

// ------------------------------------------------------------ index access

/// One cached read-only connection, replaced if the database file is
/// replaced (the daemon's corruption recovery renames it aside).
pub struct IndexAccess {
    db: PathBuf,
    cached: Mutex<Option<((u64, u64), Reader)>>,
}

impl IndexAccess {
    pub fn new(db: PathBuf) -> IndexAccess {
        IndexAccess { db, cached: Mutex::new(None) }
    }

    pub fn path(&self) -> &Path {
        &self.db
    }

    /// Client-facing error that never contains the database path (it can
    /// reveal user or directory names outside the allowlist). The detail
    /// goes to stderr, which MCP clients keep in local logs only.
    fn unavailable(&self, e: impl std::fmt::Display, missing: bool) -> String {
        eprintln!("ferroseek-mcp: index {}: {e}", self.db.display());
        if missing {
            "FerroSeek index unavailable: not found; start ferroseekd or run `ferroseek index`".into()
        } else {
            "FerroSeek index unavailable: it cannot be opened (corrupt, incompatible or being rebuilt)".into()
        }
    }

    /// Drops the cached connection (and its page cache).
    pub fn release(&self) {
        self.cached.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Runs `f` with a reader. Concurrent callers get their own temporary
    /// connection; a reader that failed is dropped.
    pub fn with<T>(&self, f: impl FnOnce(&Reader) -> ferroseek_index::Result<T>) -> Result<T, String> {
        let identity = std::fs::metadata(&self.db)
            .map(|m| (m.dev(), m.ino()))
            .map_err(|e| self.unavailable(&e, e.kind() == io::ErrorKind::NotFound))?;
        let cached = self.cached.lock().unwrap_or_else(|e| e.into_inner()).take();
        let reader = match cached {
            Some((id, r)) if id == identity => r,
            _ => Reader::open(&self.db).map_err(|e| self.unavailable(e, false))?,
        };
        let out = f(&reader).map_err(|e| {
            eprintln!("ferroseek-mcp: index query failed: {e}");
            "index query failed (the index may be busy or being rebuilt); try again".to_string()
        });
        if out.is_ok() {
            let mut slot = self.cached.lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_none() {
                *slot = Some((identity, reader));
            }
        }
        out
    }
}

// ------------------------------------------------------------ output types

/// One file or directory, as returned to agents.
#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct Entry {
    /// Absolute path. Non-UTF-8 bytes are replaced (see `non_utf8`).
    pub path: String,
    /// `file`, `directory`, `symlink` or `other`.
    pub kind: &'static str,
    /// Size in bytes (files and symlinks only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Last modification time, UTC, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// The real path contains bytes that are not UTF-8; `path` is lossy.
    /// Omitted when false (`default` keeps it optional in the schema).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub non_utf8: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    /// Best matches first.
    pub results: Vec<Entry>,
    /// More matches exist than were returned (raise `limit` or refine).
    pub more_available: bool,
    /// Results were cut to keep the response small.
    pub truncated_by_size: bool,
    /// Indexed matches dropped because they no longer exist on disk.
    pub omitted_stale: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FileInfoOutput {
    #[serde(flatten)]
    pub entry: Entry,
    /// The path still exists on disk.
    pub exists: bool,
    /// The index metadata matches the file on disk (false while a change
    /// is still being indexed, or if the file is gone).
    pub up_to_date: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AllowedDir {
    pub path: String,
    /// The directory is present in the FerroSeek index.
    pub indexed: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusOutput {
    /// The index database can be opened and queried.
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Totals for the whole index, including unauthorized directories.
    /// Only reported when `expose_index_totals = true` in mcp.toml.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_files: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_directories: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_size_bytes: Option<u64>,
    /// When the index was last written (UTC, RFC 3339); opt-in with totals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_write: Option<String>,
    /// Directories this MCP server may expose.
    pub allowed_dirs: Vec<AllowedDir>,
    pub max_results: usize,
}

// ------------------------------------------------------------ parameters

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Words that must all appear in the file name (case-insensitive
    /// substrings). A word containing '/' must appear in the full path.
    /// Example: "invoice 2024" or "src/ main.rs".
    pub query: String,
    /// Absolute directory to search in; must be an authorized directory
    /// or inside one. Default: all authorized directories.
    #[serde(default)]
    pub root: Option<String>,
    /// Maximum number of results (default 20, capped by the server).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InfoParams {
    /// Absolute path of an indexed file or directory inside an authorized
    /// directory.
    pub path: String,
}

// ------------------------------------------------------------ helpers

pub fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::File => "file",
        Kind::Dir => "directory",
        Kind::Symlink => "symlink",
        Kind::Other => "other",
    }
}

/// RFC 3339 UTC from nanoseconds since the epoch (civil-from-days).
pub fn rfc3339(ns: i64) -> Option<String> {
    if ns <= 0 {
        return None;
    }
    let secs = ns.div_euclid(1_000_000_000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60))
}

fn entry(path: &Path, meta: &Meta) -> Entry {
    let s = path.to_str();
    Entry {
        path: s.map(str::to_owned).unwrap_or_else(|| path.to_string_lossy().into_owned()),
        kind: kind_str(meta.kind),
        size: (meta.kind != Kind::Dir).then_some(meta.size),
        modified: rfc3339(meta.mtime_ns),
        non_utf8: s.is_none(),
    }
}

/// Current on-disk metadata of `p` (inside allowed dir `base`) without
/// following symlinks at any level below `base`.
fn disk_meta(base: &Path, p: &Path) -> io::Result<Meta> {
    stat_beneath(base, p.strip_prefix(base).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?)
}

/// "No longer an entry inside the allowed directory": gone, or a path
/// component was replaced by a symlink or a non-directory.
fn diverged(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound || matches!(e.raw_os_error(), Some(libc::ENOTDIR) | Some(libc::ELOOP))
}

fn same_content_meta(a: &Meta, b: &Meta) -> bool {
    // Directory mtime in the index means "listing reconciled at"; compare
    // only what is meaningful for each kind.
    a.kind == b.kind && a.ino == b.ino && (a.kind == Kind::Dir || (a.size == b.size && a.mtime_ns == b.mtime_ns))
}

fn validate_query(q: &str) -> Result<&str, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("query is empty".into());
    }
    if q.len() > policy::MAX_QUERY_BYTES {
        return Err(format!("query is longer than {} bytes", policy::MAX_QUERY_BYTES));
    }
    if q.chars().any(|c| c.is_control()) {
        return Err("query contains control characters".into());
    }
    Ok(q)
}

// ------------------------------------------------------------ core logic (sync)

/// Candidate cap per scope when hidden/denied/stale entries crowd out
/// visible ones. Beyond it, `more_available` is reported as false.
pub const MAX_CANDIDATES: usize = 8192;

/// Searches every scope, keeps only policy-visible entries that still
/// exist, and bounds the response by count and bytes.
///
/// Filtering happens *before* anything is counted: `more_available` is true
/// only if an extra authorized, existing result was found, so hidden or
/// denied matches cannot be detected through it. If filtered candidates
/// crowd the first fetch, the fetch grows (up to [`MAX_CANDIDATES`]).
pub fn search(policy: &Policy, index: &IndexAccess, p: &SearchParams) -> Result<SearchOutput, String> {
    let q = validate_query(&p.query)?;
    let scopes = policy.scopes(p.root.as_deref()).map_err(|e| e.to_string())?;
    let limit =
        p.limit.map(|l| l as usize).unwrap_or(DEFAULT_LIMIT.min(policy.max_results)).clamp(1, policy.max_results);
    let query = Query::parse(q);
    let mut fetch = limit * 2 + 16;
    let (visible, stale) = loop {
        let mut hits: Vec<Hit> = Vec::new();
        let mut saturated = false;
        index.with(|r| {
            // A scope that contains every index root needs no `within` filter
            // (which would seed a subtree walk); the checks below keep results
            // inside the scope. Several such scopes share one query.
            let roots = r.roots()?;
            let covers_all = |s: &PathBuf| !roots.is_empty() && roots.iter().all(|root| root.starts_with(s));
            let (unscoped, scoped): (Vec<&PathBuf>, Vec<&PathBuf>) = scopes.iter().partition(|s| covers_all(s));
            let mut run = |within: Option<PathBuf>| -> ferroseek_index::Result<()> {
                let opts = SearchOptions { limit: fetch, within, kind: None, include_hidden: true };
                let found = r.search(&query, &opts)?;
                saturated |= found.len() >= fetch;
                hits.extend(found);
                Ok(())
            };
            if !unscoped.is_empty() {
                run(None)?;
            }
            for s in scoped {
                run(Some(s.clone()))?;
            }
            Ok(())
        })?;
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        hits.dedup_by_key(|h| h.id);

        let mut visible: Vec<(PathBuf, Meta)> = Vec::new();
        let mut stale = 0u32;
        for h in hits {
            let Some(base) = policy.visible_under(&h.path) else { continue };
            if !scopes.iter().any(|s| h.path.starts_with(s) && h.path != *s) {
                continue;
            }
            // Report what is on disk now; drop entries that vanished or whose
            // ancestors changed since indexing. Unreadable metadata keeps the
            // indexed values.
            let meta = match disk_meta(base, &h.path) {
                Ok(m) => m,
                Err(e) if diverged(&e) => {
                    stale += 1;
                    continue;
                }
                Err(_) => Meta { kind: h.kind, size: h.size, mtime_ns: h.mtime_ns, ino: 0 },
            };
            visible.push((h.path, meta));
            if visible.len() > limit {
                break;
            }
        }
        if visible.len() > limit || !saturated || fetch >= MAX_CANDIDATES {
            break (visible, stale);
        }
        fetch = (fetch * 4).min(MAX_CANDIDATES);
    };

    let mut out = SearchOutput {
        results: Vec::new(),
        more_available: visible.len() > limit,
        truncated_by_size: false,
        omitted_stale: stale,
    };
    let mut bytes = 2usize;
    for (path, meta) in visible.into_iter().take(limit) {
        let e = entry(&path, &meta);
        let n = serde_json::to_string(&e).map(|s| s.len() + 1).unwrap_or(0);
        if bytes + n > RESULT_BYTES_BUDGET {
            out.truncated_by_size = true;
            out.more_available = true;
            break;
        }
        bytes += n;
        out.results.push(e);
    }
    Ok(out)
}

pub fn file_info(policy: &Policy, index: &IndexAccess, p: &InfoParams) -> Result<FileInfoOutput, String> {
    let path = policy::parse_client_path(&p.path).map_err(|e| e.to_string())?;
    if policy.allowed_dirs().is_empty() {
        return Err(Denied::NoAllowedDirs.to_string());
    }
    let base = if policy.allowed_dirs().contains(&path) {
        path.clone()
    } else {
        match policy.visible_under(&path) {
            Some(b) => b.to_path_buf(),
            None => return Err(Denied::NotAllowed.to_string()),
        }
    };
    let Some((_, indexed)) = index.with(|r| r.lookup(&path))? else {
        return Err("path is not in the FerroSeek index (it may not exist, be excluded, or not be indexed yet)".into());
    };
    let (exists, up_to_date, meta) = match disk_meta(&base, &path) {
        Ok(m) => (true, same_content_meta(&indexed, &m), if indexed.kind == Kind::Dir { indexed } else { m }),
        Err(_) => (false, false, indexed),
    };
    Ok(FileInfoOutput { entry: entry(&path, &meta), exists, up_to_date })
}

pub fn status(policy: &Policy, index: &IndexAccess) -> StatusOutput {
    let dirs = policy.allowed_dirs().to_vec();
    let totals = policy.expose_index_totals;
    let res = index.with(|r| {
        let stats = if totals { Some(r.stats()?) } else { None };
        let mut allowed = Vec::new();
        for d in &dirs {
            allowed.push(AllowedDir { path: d.to_string_lossy().into_owned(), indexed: r.lookup(d)?.is_some() });
        }
        Ok((stats, allowed))
    });
    let mut out = StatusOutput {
        available: false,
        error: None,
        indexed_files: None,
        indexed_directories: None,
        index_size_bytes: None,
        last_write: None,
        allowed_dirs: Vec::new(),
        max_results: policy.max_results,
    };
    match res {
        Ok((stats, allowed)) => {
            out.available = true;
            out.allowed_dirs = allowed;
            if let Some(s) = stats {
                out.indexed_files = Some(s.files);
                out.indexed_directories = Some(s.dirs);
                out.index_size_bytes = Some(s.disk_bytes);
                // Write activity anywhere in the index: opt-in like totals.
                out.last_write = ["", "-wal"]
                    .iter()
                    .filter_map(|suffix| {
                        let mut p = index.path().as_os_str().to_owned();
                        p.push(suffix);
                        std::fs::metadata(PathBuf::from(p)).ok()
                    })
                    .map(|m| m.mtime() * 1_000_000_000 + m.mtime_nsec())
                    .max()
                    .and_then(rfc3339);
            }
        }
        Err(e) => {
            out.error = Some(e);
            out.allowed_dirs =
                dirs.iter().map(|d| AllowedDir { path: d.to_string_lossy().into_owned(), indexed: false }).collect();
        }
    }
    out
}

// ------------------------------------------------------------ MCP surface

/// Quiet time after the last request before cached memory is released.
pub const RELEASE_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

struct Inner {
    policy: Policy,
    index: IndexAccess,
    activity: AtomicU64,
    wake: tokio::sync::Notify,
}

#[derive(Clone)]
pub struct FerroSeekMcp {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

impl FerroSeekMcp {
    pub fn new(policy: Policy, index: IndexAccess) -> Self {
        let inner = Inner { policy, index, activity: AtomicU64::new(0), wake: tokio::sync::Notify::new() };
        FerroSeekMcp { inner: Arc::new(inner), tool_router: Self::tool_router() }
    }

    /// Releases the cached reader and returns freed heap to the kernel
    /// once requests stop for [`RELEASE_AFTER`]. Between bursts it waits on
    /// a `Notify`: no timers, no wakeups while idle. Call inside the runtime.
    pub fn spawn_idle_release(&self) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut released_at = 0;
            loop {
                inner.wake.notified().await;
                // A permit stored during the last burst must not start another
                // cycle when nothing happened since the last release.
                if inner.activity.load(Ordering::Relaxed) == released_at {
                    continue;
                }
                loop {
                    let seen = inner.activity.load(Ordering::Relaxed);
                    tokio::time::sleep(RELEASE_AFTER).await;
                    if inner.activity.load(Ordering::Relaxed) == seen {
                        break;
                    }
                }
                released_at = inner.activity.load(Ordering::Relaxed);
                inner.index.release();
                ferroseek_engine::sys::trim_heap();
            }
        });
    }

    /// Index work is blocking SQLite I/O: keep it off the async thread.
    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Policy, &IndexAccess) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let inner = self.inner.clone();
        let out = tokio::task::spawn_blocking(move || f(&inner.policy, &inner.index))
            .await
            .unwrap_or_else(|e| Err(format!("internal error: {e}")));
        self.inner.activity.fetch_add(1, Ordering::Relaxed);
        self.inner.wake.notify_one();
        out
    }
}

#[tool_router]
impl FerroSeekMcp {
    /// Find files and directories by name in the user's FerroSeek index.
    #[tool(
        description = "Search indexed file and directory names (case-insensitive substrings; all words must \
                       match; a word containing '/' matches the path). Only directories the user authorized \
                       are searched. Returns paths with kind, size and modification time; does not read files.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_files(&self, Parameters(p): Parameters<SearchParams>) -> Result<Json<SearchOutput>, String> {
        self.blocking(move |pol, idx| search(pol, idx, &p)).await.map(Json)
    }

    /// Metadata of one indexed path.
    #[tool(
        description = "Get kind, size and modification time of one indexed file or directory inside an \
                       authorized directory, and whether the index is up to date for it. Does not read content.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_file_info(&self, Parameters(p): Parameters<InfoParams>) -> Result<Json<FileInfoOutput>, String> {
        self.blocking(move |pol, idx| file_info(pol, idx, &p)).await.map(Json)
    }

    /// Index availability and statistics.
    #[tool(
        description = "Report whether the FerroSeek index is available, its size and last update, and which \
                       directories this server may search.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn index_status(&self) -> Result<Json<StatusOutput>, String> {
        self.blocking(|pol, idx| Ok(status(pol, idx))).await.map(Json)
    }
}

#[tool_handler(
    router = self.tool_router,
    name = "ferroseek",
    instructions = "Read-only file-name search over the user's local FerroSeek index. Use search_files to \
                    locate files by name or path fragments, get_file_info for metadata of a known path and \
                    index_status to see which directories are authorized. File contents are not available."
)]
impl ServerHandler for FerroSeekMcp {}
