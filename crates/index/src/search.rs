// SPDX-License-Identifier: GPL-3.0-or-later
//! Query parsing, candidate generation and ranking.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension};

use crate::tree::fold;
use crate::{Id, Kind, Result};

/// Lower bound on FTS candidates per query; the effective cap is
/// `max(FTS_CANDIDATES, 4 * limit)`. Bounds latency on unselective queries
/// for small limits (the launcher asks for 15) while large limits (CLI scripting)
/// retain more matches. Filters run before these budgets; selective filters
/// may read many rows. Exact and prefix candidates are fetched separately.
const FTS_CANDIDATES: usize = 2000;
const PREFIX_CANDIDATES: usize = 300;

/// A parsed query.
///
/// Whitespace separates tokens; all must match (case-insensitive):
/// - a token containing `/` must be a substring of the full path;
/// - any other token must be a substring of the file name. Name matching
///   uses the trigram index for tokens of three or more characters; a query
///   made only of shorter tokens matches name *prefixes*.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    name_tokens: Vec<String>,
    path_tokens: Vec<String>,
}

impl Query {
    pub fn parse(q: &str) -> Query {
        if q.contains('\0') {
            return Query::default();
        }
        let mut out = Query::default();
        for t in q.split_whitespace() {
            let t = fold(t.as_bytes());
            if t.contains('/') {
                out.path_tokens.push(t);
            } else {
                out.name_tokens.push(t);
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.name_tokens.is_empty() && self.path_tokens.iter().all(|t| t.chars().all(|c| c == '/'))
    }
}

/// A caller-supplied predicate on full paths (e.g. an access policy).
#[derive(Clone)]
pub struct PathFilter(pub std::sync::Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync>);

impl std::fmt::Debug for PathFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PathFilter(..)")
    }
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub limit: usize,
    /// Only return entries strictly below this absolute directory.
    pub within: Option<PathBuf>,
    /// Restrict to one kind (e.g. only directories).
    pub kind: Option<Kind>,
    /// Include dotfiles and entries below dot-directories.
    pub include_hidden: bool,
    /// Only return paths this predicate accepts. It runs where `within` and
    /// the hidden filter run, *before* candidate budgets, so rejected
    /// entries never use up the budget. `None` (the default) changes
    /// nothing.
    pub accept: Option<PathFilter>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions { limit: 20, within: None, kind: None, include_hidden: true, accept: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: Id,
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime_ns: i64,
    /// Higher is better. Unbounded; see [`Hit::relevance`].
    pub score: f32,
}

impl Hit {
    /// Score mapped into (0, 1), e.g. for the launcher.
    pub fn relevance(&self) -> f64 {
        let s = self.score.max(0.0) as f64;
        (s / (s + 60.0)).clamp(0.01, 0.99)
    }
}

struct Cand {
    parent: Option<Id>,
    name: Vec<u8>,
    key: String,
    kind: Kind,
    size: u64,
    mtime: i64,
    score: f32,
}

fn fts_phrase(tokens: &[&String]) -> String {
    tokens.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" ")
}

fn is_boundary(key: &str, name: &[u8], pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    let prev = key[..pos].chars().next_back().unwrap_or(' ');
    if !prev.is_alphanumeric() {
        return true;
    }
    // camelCase boundary in the original name ("myReport" → "Report").
    if name.len() == key.len() && pos < name.len() {
        let (p, c) = (name[pos - 1], name[pos]);
        return p.is_ascii_lowercase() && c.is_ascii_uppercase();
    }
    false
}

/// Name-only score. Exact name > exact stem > prefix > word boundary >
/// inner substring, plus how much of the name the query covers.
fn name_score(key: &str, name: &[u8], tokens: &[String]) -> f32 {
    let mut s = 0.0;
    let joined = tokens.join(" ");
    let stem = match key.rfind('.') {
        Some(p) if p > 0 => &key[..p],
        _ => key,
    };
    if key == joined {
        s += 100.0;
    } else if stem == joined {
        s += 80.0;
    }
    let mut covered = 0usize;
    for t in tokens {
        if let Some(p) = key.find(t.as_str()) {
            covered += t.chars().count();
            s += if p == 0 {
                30.0
            } else if is_boundary(key, name, p) {
                20.0
            } else {
                8.0
            };
        }
    }
    let len = key.chars().count().max(1);
    s += 25.0 * (covered.min(len) as f32 / len as f32);
    if key.starts_with('.') {
        s -= 15.0;
    }
    s
}

pub(crate) fn run(conn: &Connection, q: &Query, opts: &SearchOptions) -> Result<Vec<Hit>> {
    if q.is_empty() || opts.limit == 0 {
        return Ok(Vec::new());
    }
    let scope = directory_scope(conn, q, opts)?;
    let restriction = scope.map(|s| format!(" AND e.parent IN ({s})")).unwrap_or_default();
    let mut cands: HashMap<Id, Cand> = HashMap::new();
    let mut candidate_paths = PathCache::new(conn);
    let mut add_rows =
        |st: &mut rusqlite::CachedStatement<'_>, params: &[&dyn rusqlite::ToSql], cap: usize| -> Result<()> {
            let mut accepted = 0;
            let mut rows = st.query(params)?;
            while let Some(r) = rows.next()? {
                let id: Id = r.get(0)?;
                let c = Cand {
                    parent: r.get(1)?,
                    name: r.get(2)?,
                    key: r.get(3)?,
                    kind: Kind::from_i64(r.get(4)?),
                    size: r.get::<_, i64>(5)? as u64,
                    mtime: r.get(6)?,
                    score: 0.0,
                };
                if !q.name_tokens.iter().all(|t| c.key.contains(t.as_str())) || opts.kind.is_some_and(|k| k != c.kind) {
                    continue;
                }
                let Some(parent) = c.parent else { continue };
                if !q.path_tokens.is_empty() || opts.within.is_some() || !opts.include_hidden || opts.accept.is_some() {
                    let Some(dir) = candidate_paths.dir(parent)? else { continue };
                    let path = PathBuf::from(OsStr::from_bytes(&dir.path)).join(OsStr::from_bytes(&c.name));
                    if !path_matches(&path, dir.hidden || c.name.first() == Some(&b'.'), q, opts) {
                        continue;
                    }
                }
                cands.entry(id).or_insert(c);
                accepted += 1;
                if accepted >= cap {
                    break;
                }
            }
            Ok(())
        };
    const COLS: &str = "e.id, e.parent, e.name, e.key, e.kind, e.size, e.mtime";

    let long: Vec<&String> = q.name_tokens.iter().filter(|t| t.chars().count() >= 3).collect();
    let fts_cap = FTS_CANDIDATES.max(opts.limit.saturating_mul(4));
    let prefix_cap = PREFIX_CANDIDATES.max(opts.limit);
    let small_scope = if !restriction.is_empty() {
        let sql = format!("SELECT count(*) FROM (SELECT 1 FROM entries e WHERE 1{restriction} LIMIT 4097)");
        conn.query_row(&sql, [], |r| r.get::<_, i64>(0))? <= 4096
    } else {
        false
    };
    if small_scope {
        let prefix = if long.is_empty() {
            q.name_tokens
                .first()
                .map(|s| {
                    format!(
                        " AND e.key >= '{}' AND e.key < '{}'",
                        s.replace('\'', "''"),
                        format!("{s}\u{10ffff}").replace('\'', "''")
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        add_rows(
            &mut conn
                .prepare_cached(&format!("SELECT {COLS} FROM entries e WHERE 1{restriction}{prefix} ORDER BY e.id"))?,
            &[],
            4096,
        )?;
    } else {
        if let Some(first) = q.name_tokens.first() {
            // Fetch exact and prefix candidates separately using index ranges.
            let joined = q.name_tokens.join(" ");
            add_rows(
                &mut conn.prepare_cached(&format!(
                    "SELECT {COLS} FROM entries e WHERE e.key = ?1{restriction} ORDER BY e.id"
                ))?,
                &[&joined],
                100,
            )?;
            let upper = format!("{first}\u{10FFFF}");
            add_rows(
                &mut conn.prepare_cached(&format!(
                    "SELECT {COLS} FROM entries e WHERE e.key >= ?1 AND e.key < ?2{restriction} ORDER BY e.key, e.id"
                ))?,
                &[first, &upper],
                prefix_cap,
            )?;
        } else {
            // No filename constraint can safely narrow a full-path substring.
            // Stream rows and retain only the bounded set passing path filters.
            add_rows(
                &mut conn
                    .prepare_cached(&format!("SELECT {COLS} FROM entries e WHERE 1{restriction} ORDER BY e.id"))?,
                &[],
                fts_cap,
            )?;
        }
        if !long.is_empty() {
            add_rows(
                &mut conn.prepare_cached(&format!(
                    "SELECT {COLS} FROM names JOIN entries e ON e.id = names.rowid
                 WHERE names MATCH ?1{restriction} ORDER BY names.rowid"
                ))?,
                &[&fts_phrase(&long)],
                fts_cap,
            )?;
        }
    }

    // Verify every name token (FTS is exact for trigrams, but short tokens
    // and the prefix/exact fetches need it) and score by name.
    let mut ranked: Vec<(Id, Cand)> = cands
        .into_iter()
        .filter(|(_, c)| q.name_tokens.iter().all(|t| c.key.contains(t.as_str())))
        .filter(|(_, c)| opts.kind.is_none_or(|k| k == c.kind))
        .filter(|(_, c)| c.parent.is_some()) // roots are not results
        .map(|(id, mut c)| {
            c.score = name_score(&c.key, &c.name, &q.name_tokens);
            (id, c)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0)));

    // Stage 2 needs paths. Filters that depend on the path must see every
    // candidate; otherwise only the head of the name ranking matters.
    let path_filtered =
        !q.path_tokens.is_empty() || opts.within.is_some() || !opts.include_hidden || opts.accept.is_some();
    // `accept` alone already ran on every candidate before its budget (see
    // add_rows). Instead of truncating the name ranking (which can drop the
    // best *final* score), such searches stop early with an exact bound: the
    // only positive adjustment below is the recency bonus, so a candidate
    // can never end above `name score + MAX_BONUS`.
    let accept_only = opts.accept.is_some() && q.path_tokens.is_empty() && opts.within.is_none() && opts.include_hidden;
    if !path_filtered {
        ranked.truncate((opts.limit.saturating_mul(3)).max(60));
    }
    const MAX_BONUS: f32 = 6.0;
    // Min-heap of the best `limit` final scores so far (accept-only path).
    let mut best: std::collections::BinaryHeap<std::cmp::Reverse<TotalF32>> = std::collections::BinaryHeap::new();
    // Reuse the directory paths resolved while collecting candidates.
    let mut paths = candidate_paths;
    let now_ns =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0);
    let mut hits = Vec::with_capacity(ranked.len().min(opts.limit.saturating_mul(3)));
    for (id, c) in ranked {
        if accept_only && best.len() >= opts.limit && best.peek().is_some_and(|k| c.score + MAX_BONUS < k.0.0) {
            // `ranked` is sorted by name score: no later candidate can enter
            // the top `limit` (ties are kept: the comparison is strict).
            break;
        }
        let Some(parent) = c.parent else { continue };
        let Some(dir) = paths.dir(parent)? else { continue };
        let mut path = dir.path.to_vec();
        if path.last() != Some(&b'/') {
            path.push(b'/');
        }
        path.extend_from_slice(&c.name);
        let path = PathBuf::from(OsStr::from_bytes(&path));
        let hidden_ancestor = dir.hidden;
        if !path_matches(&path, hidden_ancestor || c.name.first() == Some(&b'.'), q, opts) {
            continue;
        }
        let mut score = c.score;
        // Files directly in a root or one level down are not penalized.
        score -= dir.depth.saturating_sub(1) as f32;
        if hidden_ancestor {
            score -= 10.0;
        }
        let age_days = (now_ns - c.mtime) as f64 / 86_400e9;
        if c.mtime > 0 && age_days >= 0.0 {
            if age_days < 7.0 {
                score += MAX_BONUS;
            } else if age_days < 30.0 {
                score += 3.0;
            }
        }
        if accept_only {
            best.push(std::cmp::Reverse(TotalF32(score)));
            if best.len() > opts.limit {
                best.pop();
            }
        }
        hits.push(Hit { id, path, kind: c.kind, size: c.size, mtime_ns: c.mtime, score });
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(opts.limit);
    Ok(hits)
}

/// Restrict candidates to descendants of a necessary directory component.
/// Full path verification still runs: this is a superset, not the matcher.
fn directory_scope(conn: &Connection, q: &Query, opts: &SearchOptions) -> Result<Option<String>> {
    let seed = if let Some(within) = &opts.within {
        let mut ids = Vec::new();
        if let Some((id, meta)) = crate::tree::lookup_path(conn, within)?
            && meta.kind == Kind::Dir
        {
            ids.push(id);
        }
        for (id, path) in crate::tree::roots(conn)? {
            if path.starts_with(within) {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return Ok(Some("SELECT NULL WHERE 0".into()));
        }
        format!(
            "SELECT id FROM entries WHERE id IN ({})",
            ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
        )
    } else {
        let anchor = q
            .path_tokens
            .iter()
            .flat_map(|p| {
                let (dirs, _) = p.rsplit_once('/').unwrap();
                dirs.split('/')
            })
            .filter(|p| p.chars().count() >= 3)
            .max_by_key(|p| p.chars().count());
        let Some(anchor) = anchor else { return Ok(None) };
        // Quote separately for the FTS phrase grammar and SQL string grammar.
        let phrase = format!("\"{}\"", anchor.replace('"', "\"\""));
        format!(
            "SELECT e.id FROM names JOIN entries e ON e.id=names.rowid WHERE names MATCH '{}' AND e.kind=1",
            phrase.replace('\'', "''")
        )
    };
    // Do not expand a root or a broad set of directories just to discard
    // the scope later. These cases retain the original streaming strategy.
    let mut seeds = conn.prepare(&format!("{seed} LIMIT 257"))?;
    let ids: Vec<Id> = seeds.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    if ids.len() > 256 || crate::tree::roots(conn)?.iter().any(|(id, _)| ids.contains(id)) {
        return Ok(None);
    }
    if ids.is_empty() {
        return Ok(Some("SELECT NULL WHERE 0".into()));
    }
    let seed_ids = ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
    let sql = format!(
        "WITH RECURSIVE dirs(id) AS (SELECT id FROM entries WHERE id IN ({seed_ids}) UNION SELECT e.id FROM entries e INDEXED BY entries_parent_name JOIN dirs ON e.parent=dirs.id WHERE e.kind=1 LIMIT 257) SELECT id FROM dirs"
    );
    let mut st = conn.prepare(&sql)?;
    let dirs: Vec<Id> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    if dirs.len() > 256 {
        return Ok(None);
    }
    Ok(Some(format!(
        "SELECT id FROM entries WHERE id IN ({})",
        dirs.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
    )))
}

/// `f32` ordered by `total_cmp`, for the top-k heap.
#[derive(Clone, Copy, PartialEq)]
struct TotalF32(f32);
impl Eq for TotalF32 {}
impl PartialOrd for TotalF32 {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for TotalF32 {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0)
    }
}

fn path_matches(path: &std::path::Path, hidden: bool, q: &Query, opts: &SearchOptions) -> bool {
    if opts.within.as_ref().is_some_and(|w| !path.starts_with(w) || path == w) {
        return false;
    }
    if hidden && !opts.include_hidden {
        return false;
    }
    if opts.accept.as_ref().is_some_and(|f| !(f.0)(path)) {
        return false;
    }
    if !q.path_tokens.is_empty() {
        let lower = fold(path.as_os_str().as_bytes());
        if !q.path_tokens.iter().all(|t| lower.contains(t.as_str())) {
            return false;
        }
    }
    true
}

/// A resolved directory: absolute path, plus facts measured *below its
/// indexing root* (a root under `/tmp/.x` must not make everything hidden).
#[derive(Clone)]
struct DirInfo {
    path: std::rc::Rc<[u8]>,
    /// Some directory between the root (exclusive) and here is a dotdir.
    hidden: bool,
    /// Directories between the root and here (root itself = 0).
    depth: u32,
}

/// Per-query cache of directory info: candidates share parents heavily.
struct PathCache<'c> {
    conn: &'c Connection,
    dirs: HashMap<Id, Option<DirInfo>>,
}

impl<'c> PathCache<'c> {
    fn new(conn: &'c Connection) -> Self {
        PathCache { conn, dirs: HashMap::new() }
    }

    fn dir(&mut self, id: Id) -> Result<Option<DirInfo>> {
        if let Some(p) = self.dirs.get(&id) {
            return Ok(p.clone());
        }
        // Candidate filtering may visit many rejected paths. Keep the
        // cache bounded independently of the number of rows scanned.
        if self.dirs.len() >= 8192 {
            self.dirs.clear();
        }
        // Walk up until a cached ancestor or a root, then fill the cache on
        // the way back down.
        let mut chain: Vec<(Id, Vec<u8>)> = Vec::new();
        let mut cur = id;
        let mut st = self.conn.prepare_cached("SELECT parent, name FROM entries WHERE id = ?1")?;
        let mut base: Option<DirInfo> = None;
        loop {
            if let Some(p) = self.dirs.get(&cur) {
                match p {
                    Some(info) => base = Some(info.clone()),
                    None => return Ok(None),
                }
                break;
            }
            let row: Option<(Option<Id>, Vec<u8>)> = st.query_row([cur], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let Some((parent, name)) = row else {
                self.dirs.insert(id, None);
                return Ok(None);
            };
            chain.push((cur, name));
            match parent {
                Some(p) => cur = p,
                None => break,
            }
            if chain.len() > 4096 {
                return Ok(None);
            }
        }
        let mut acc = base;
        while let Some((cid, name)) = chain.pop() {
            let info = match acc {
                // A root: its name is the absolute path.
                None => DirInfo { path: name.into(), hidden: false, depth: 0 },
                Some(parent) => {
                    let mut p = parent.path.to_vec();
                    if p.last() != Some(&b'/') {
                        p.push(b'/');
                    }
                    p.extend_from_slice(&name);
                    DirInfo {
                        path: p.into(),
                        hidden: parent.hidden || name.first() == Some(&b'.'),
                        depth: parent.depth + 1,
                    }
                }
            };
            self.dirs.insert(cid, Some(info.clone()));
            acc = Some(info);
        }
        Ok(self.dirs.get(&id).cloned().flatten())
    }
}
