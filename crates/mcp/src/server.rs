// SPDX-License-Identifier: GPL-3.0-or-later
//! The MCP tools. Every response is built from the existing index through a
//! read-only [`Reader`] and filtered by the [`Policy`] before it leaves the
//! process.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferroseek_index::{Hit, Kind, Meta, PathFilter, Query, Reader, SearchOptions};
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
    /// Lower bound on stale indexed matches observed and discarded.
    pub omitted_stale: u32,
    /// Concurrent filesystem changes exhausted bounded retry work; refine or retry.
    pub incomplete_due_to_changes: bool,
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
    pub access_mode: String,
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
    /// Filename or partial path to find, not file contents. Space-separated
    /// words are case-insensitive substrings that must all match: words with
    /// '/' match the full path, other words match the name. No glob or regex
    /// syntax. Examples: "invoice 2024", "src/ main.rs", "services/payments/".
    pub query: String,
    /// Optional absolute directory to narrow the search; omit for discovery
    /// across all authorized indexed directories and repositories. Must be
    /// authorized; cannot expand index coverage. No '.' or '..' components.
    #[serde(default)]
    pub root: Option<String>,
    /// Maximum results (default 20, server policy cap, hard maximum 200).
    /// Refine the query or increase this if more_available is true.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InfoParams {
    /// Absolute authorized indexed path, typically returned by search_files.
    /// Not a general filesystem stat: unknown or unauthorized paths are refused.
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

/// Retry cap for paths changing between candidate validation and response
/// construction. Stable deleted entries are filtered before the index budget.
pub const MAX_CANDIDATES: usize = 8192;
/// Upper bound on memoized parent-directory verdicts per search.
const MEMO_DIRS: usize = 4096;

/// Searches every scope, keeps only policy-visible entries that still
/// exist, and bounds the response by count and bytes.
///
/// The access policy and the scope check run *inside* the index search
/// (`SearchOptions::accept`), before its candidate budget: hidden, denied or
/// out-of-scope entries can neither be observed (`more_available` counts
/// only authorized, existing results) nor crowd out visible ones. Only
/// stale entries trigger a second query with disk validation inside the
/// candidate budget, so an arbitrarily long deleted prefix cannot starve it.
pub fn search(policy: &Policy, index: &IndexAccess, p: &SearchParams) -> Result<SearchOutput, String> {
    let q = validate_query(&p.query)?;
    let scopes = policy.scopes(p.root.as_deref()).map_err(|e| e.to_string())?;
    let limit =
        p.limit.map(|l| l as usize).unwrap_or(DEFAULT_LIMIT.min(policy.max_results)).clamp(1, policy.max_results);
    let query = Query::parse(q);
    let accept = {
        // `path` is visible and in scope iff its parent admits children (is
        // an allowed dir or visible, and is a scope or inside one) and its
        // own name passes. The parent verdict is memoized: candidates share
        // a few thousand directories at most, and re-checking every ancestor
        // per candidate dominated latency (1M: p50 7.0 ms → see BENCHMARKS).
        let (policy, scopes) = (policy.clone(), scopes.clone());
        let memo: Mutex<std::collections::HashMap<PathBuf, bool>> = Mutex::new(Default::default());
        PathFilter(Arc::new(move |path: &Path| {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return false };
            if !policy.name_ok(name) {
                return false;
            }
            let mut memo = memo.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = memo.get(parent) {
                return *v;
            }
            let v = policy.dir_admits(parent) && scopes.iter().any(|s| parent.starts_with(s));
            // Bounded: a broad query over many excluded directories must not
            // grow memory without limit; recomputing is always correct.
            if memo.len() >= MEMO_DIRS {
                memo.clear();
            }
            memo.insert(parent.to_path_buf(), v);
            v
        }))
    };
    let mut live_filter = false;
    let mut observed_stale = 0;
    let mut fetch = limit + 1;
    // Per scope (None = one unscoped query): last results and whether the
    // index returned a full page (more may exist).
    let mut pages: Vec<(Option<PathBuf>, Vec<Hit>, bool)> = Vec::new();
    let (visible, incomplete_due_to_changes) = loop {
        let candidate_filter = if live_filter {
            let lexical = accept.clone();
            let policy = policy.clone();
            PathFilter(Arc::new(move |path: &Path| {
                if !(lexical.0)(path) {
                    return false;
                }
                let Some(base) = policy.visible_under(path) else { return false };
                !disk_meta(base, path).is_err_and(|e| diverged(&e))
            }))
        } else {
            accept.clone()
        };
        index.with(|r| {
            if pages.is_empty() {
                // A scope that contains every index root needs no `within`
                // (which would seed a subtree walk); `accept` keeps results in
                // scope. Several such scopes share one query.
                let roots = r.roots()?;
                let covers_all = |s: &PathBuf| !roots.is_empty() && roots.iter().all(|root| root.starts_with(s));
                if scopes.iter().any(covers_all) {
                    pages.push((None, Vec::new(), true));
                }
                pages.extend(scopes.iter().filter(|s| !covers_all(s)).map(|s| (Some(s.clone()), Vec::new(), true)));
            }
            for (within, hits, more) in pages.iter_mut().filter(|p| p.2) {
                let opts = SearchOptions {
                    limit: fetch,
                    within: within.clone(),
                    kind: None,
                    include_hidden: true,
                    accept: Some(candidate_filter.clone()),
                };
                *hits = r.search(&query, &opts)?;
                *more = hits.len() >= fetch;
            }
            Ok(())
        })?;
        let mut hits: Vec<&Hit> = pages.iter().flat_map(|p| p.1.iter()).collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        hits.dedup_by_key(|h| h.id);

        let mut visible: Vec<(PathBuf, Meta)> = Vec::new();
        let mut stale = 0u32;
        for h in hits {
            // Defence in depth: `accept` already enforced both checks.
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
            visible.push((h.path.clone(), meta));
            if visible.len() > limit {
                break;
            }
        }
        observed_stale = observed_stale.max(stale);
        let saturated = pages.iter().any(|p| p.2);
        if visible.len() > limit || !saturated {
            break (visible, false);
        }
        if !live_filter && stale > 0 {
            // Deleted matches must not consume the index candidate budget.
            // Pay per-candidate filesystem checks only on this recovery path.
            live_filter = true;
            pages.clear();
            continue;
        }
        if fetch >= MAX_CANDIDATES {
            break (visible, true);
        }
        fetch = (fetch * 4).min(MAX_CANDIDATES);
    };

    let mut out = SearchOutput {
        results: Vec::new(),
        more_available: visible.len() > limit,
        truncated_by_size: false,
        omitted_stale: observed_stale,
        incomplete_due_to_changes,
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
        access_mode: policy.access_mode.name().into(),
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
        description = "Find files and directories by filename or partial path, especially across multiple \
                       repositories or when their location is unknown. Searches an existing local index \
                       without walking each repository; only authorized indexed paths are visible. \
                       No file-content search: use rg for text in source files. For new or unindexed files, \
                       a complete live listing, or a simple known-directory lookup, fd, rg --files or direct \
                       filesystem tools may be preferable. Missing results do not prove a file is absent: \
                       indexing can lag and exclusions apply. Returns paths, kind, size and modification time; \
                       deleted entries are filtered. Query words are literal case-insensitive substrings, \
                       not glob/regex patterns; words containing '/' match paths. No status call is required first.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_files(&self, Parameters(p): Parameters<SearchParams>) -> Result<Json<SearchOutput>, String> {
        self.blocking(move |pol, idx| search(pol, idx, &p)).await.map(Json)
    }

    /// Metadata of one indexed path.
    #[tool(
        description = "Inspect metadata and freshness of a known authorized indexed path, typically from \
                       search_files: kind, size, modification time, exists and up_to_date. Does not read \
                       file contents or follow symlinks. Not a general filesystem stat: for a new or \
                       unindexed path use direct filesystem tools within the user's permitted scope.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_file_info(&self, Parameters(p): Parameters<InfoParams>) -> Result<Json<FileInfoOutput>, String> {
        self.blocking(move |pol, idx| file_info(pol, idx, &p)).await.map(Json)
    }

    /// Index availability and statistics.
    #[tool(
        description = "Diagnose index availability and MCP authorization when a search fails or its scope \
                       is unclear. Reports access mode, authorized boundaries and result cap; whole-index \
                       totals, size and last write time appear only if enabled by policy. Authorization \
                       is not indexed coverage: an allowed boundary need not itself be indexed. Does not \
                       enumerate all indexed roots or guarantee freshness/completeness. Optional, not a \
                       prerequisite for search_files.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn index_status(&self) -> Result<Json<StatusOutput>, String> {
        self.blocking(|pol, idx| Ok(status(pol, idx))).await.map(Json)
    }
}

#[tool_handler(
    router = self.tool_router,
    name = "ferroseek",
    instructions = "Find files by name or partial path across indexed directories and repositories with \
                    search_files, without walking each repository. Authorized indexed paths only; no \
                    file-content search. Use rg for text; fd, rg --files or direct filesystem tools for \
                    new/unindexed files or complete live listings. Empty results do not prove absence. \
                    get_file_info checks known indexed paths; index_status diagnoses availability and \
                    authorization, not full coverage. Read-only, local stdio."
)]
impl ServerHandler for FerroSeekMcp {}
