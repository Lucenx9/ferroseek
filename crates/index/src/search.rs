// SPDX-License-Identifier: GPL-3.0-or-later
//! Query parsing, candidate generation and ranking.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension};

use crate::tree::fold;
use crate::{Id, Kind, Result};

/// Upper bound on FTS candidates per query. Bounds latency on unselective
/// queries; exact and prefix matches (the best-ranked classes) are fetched
/// separately and are never cut by it.
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
        let mut out = Query::default();
        for t in q.split_whitespace() {
            let t = t.to_lowercase();
            if t.contains('/') {
                out.path_tokens.push(t);
            } else {
                out.name_tokens.push(t);
            }
        }
        // "src/main" alone: also require the last component in the name, so
        // the trigram index can be used.
        if out.name_tokens.is_empty() {
            if let Some(last) = out.path_tokens.last().and_then(|p| p.rsplit('/').find(|s| !s.is_empty())) {
                out.name_tokens.push(last.to_string());
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.name_tokens.is_empty() && self.path_tokens.is_empty()
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
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions { limit: 20, within: None, kind: None, include_hidden: true }
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
    /// Score mapped into (0, 1), e.g. for KRunner.
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
    let mut cands: HashMap<Id, Cand> = HashMap::new();
    let mut add_rows = |st: &mut rusqlite::CachedStatement<'_>, params: &[&dyn rusqlite::ToSql]| -> Result<()> {
        let mut rows = st.query(params)?;
        while let Some(r) = rows.next()? {
            let id: Id = r.get(0)?;
            cands.entry(id).or_insert(Cand {
                parent: r.get(1)?,
                name: r.get(2)?,
                key: r.get(3)?,
                kind: Kind::from_i64(r.get(4)?),
                size: r.get::<_, i64>(5)? as u64,
                mtime: r.get(6)?,
                score: 0.0,
            });
        }
        Ok(())
    };
    const COLS: &str = "e.id, e.parent, e.name, e.key, e.kind, e.size, e.mtime";

    let long: Vec<&String> = q.name_tokens.iter().filter(|t| t.chars().count() >= 3).collect();
    let first = &q.name_tokens[0];
    // Exact name and prefix of the first token: cheap index range scans that
    // guarantee the best-ranked matches are always candidates.
    let joined = q.name_tokens.join(" ");
    add_rows(
        &mut conn.prepare_cached(&format!("SELECT {COLS} FROM entries e WHERE e.key = ?1 LIMIT 100"))?,
        &[&joined],
    )?;
    let upper = format!("{first}\u{10FFFF}");
    add_rows(
        &mut conn.prepare_cached(&format!(
            "SELECT {COLS} FROM entries e WHERE e.key >= ?1 AND e.key < ?2 LIMIT {PREFIX_CANDIDATES}"
        ))?,
        &[first, &upper],
    )?;
    if !long.is_empty() {
        add_rows(
            &mut conn.prepare_cached(&format!(
                "SELECT {COLS} FROM names JOIN entries e ON e.id = names.rowid
                 WHERE names MATCH ?1 LIMIT {FTS_CANDIDATES}"
            ))?,
            &[&fts_phrase(&long)],
        )?;
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
    let path_filtered = !q.path_tokens.is_empty() || opts.within.is_some() || !opts.include_hidden;
    if !path_filtered {
        ranked.truncate((opts.limit * 3).max(60));
    }
    let mut paths = PathCache::new(conn);
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    let mut hits = Vec::with_capacity(ranked.len().min(opts.limit * 3));
    for (id, c) in ranked {
        let Some(parent) = c.parent else { continue };
        let Some(dir) = paths.dir(parent)? else { continue };
        let mut path = dir.as_ref().to_vec();
        if path.last() != Some(&b'/') {
            path.push(b'/');
        }
        path.extend_from_slice(&c.name);
        let path = PathBuf::from(OsStr::from_bytes(&path));
        if let Some(w) = &opts.within {
            if !path.starts_with(w) || path == *w {
                continue;
            }
        }
        let hidden_ancestor = dir.split(|b| *b == b'/').any(|seg| seg.first() == Some(&b'.'));
        if !opts.include_hidden && (hidden_ancestor || c.name.first() == Some(&b'.')) {
            continue;
        }
        if !q.path_tokens.is_empty() {
            let lower = fold(path.as_os_str().as_bytes());
            if !q.path_tokens.iter().all(|t| lower.contains(t.as_str())) {
                continue;
            }
        }
        let mut score = c.score;
        let depth = dir.iter().filter(|b| **b == b'/').count() as f32;
        score -= (depth - 3.0).max(0.0) * 1.0;
        if hidden_ancestor {
            score -= 10.0;
        }
        let age_days = (now_ns - c.mtime) as f64 / 86_400e9;
        if c.mtime > 0 && age_days >= 0.0 {
            if age_days < 7.0 {
                score += 6.0;
            } else if age_days < 30.0 {
                score += 3.0;
            }
        }
        hits.push(Hit { id, path, kind: c.kind, size: c.size, mtime_ns: c.mtime, score });
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(opts.limit);
    Ok(hits)
}

/// Per-query cache of directory paths: candidates share parents heavily.
struct PathCache<'c> {
    conn: &'c Connection,
    dirs: HashMap<Id, Option<std::rc::Rc<[u8]>>>,
}

impl<'c> PathCache<'c> {
    fn new(conn: &'c Connection) -> Self {
        PathCache { conn, dirs: HashMap::new() }
    }

    fn dir(&mut self, id: Id) -> Result<Option<std::rc::Rc<[u8]>>> {
        if let Some(p) = self.dirs.get(&id) {
            return Ok(p.clone());
        }
        // Walk up until a cached ancestor or a root, then fill the cache on
        // the way back down.
        let mut chain: Vec<(Id, Vec<u8>)> = Vec::new();
        let mut cur = id;
        let mut st = self.conn.prepare_cached("SELECT parent, name FROM entries WHERE id = ?1")?;
        let mut base: Option<std::rc::Rc<[u8]>> = None;
        loop {
            if let Some(p) = self.dirs.get(&cur) {
                base = p.clone();
                if base.is_none() {
                    return Ok(None);
                }
                break;
            }
            let row: Option<(Option<Id>, Vec<u8>)> =
                st.query_row([cur], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
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
        let mut acc: Vec<u8> = base.map(|b| b.to_vec()).unwrap_or_default();
        while let Some((cid, name)) = chain.pop() {
            if acc.is_empty() {
                acc = name; // a root: absolute path
            } else {
                if acc.last() != Some(&b'/') {
                    acc.push(b'/');
                }
                acc.extend_from_slice(&name);
            }
            self.dirs.insert(cid, Some(acc.clone().into()));
        }
        Ok(self.dirs.get(&id).cloned().flatten())
    }
}
