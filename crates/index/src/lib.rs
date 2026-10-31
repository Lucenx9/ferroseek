// SPDX-License-Identifier: GPL-3.0-or-later
//! FerroSeek's persistent file index.
//!
//! One SQLite database holds the directory tree (parent pointers, raw byte
//! names, change-detection metadata) and an FTS5 trigram index over
//! lowercased names. See `docs/ARCHITECTURE.md` for the evidence behind this
//! choice.
//!
//! - [`Writer`] owns the only read-write connection and hands out [`Txn`]s
//!   that mutate the tree. Exactly one writer per database.
//! - [`Reader`] is a cheap read-only connection for [`Reader::search`] and
//!   lookups. Any number may coexist with the writer (WAL mode).

mod schema;
mod search;
mod tree;

use std::fmt;
use std::path::{Path, PathBuf};

pub use schema::{OpenOutcome, SCHEMA_VERSION};
pub use search::{Hit, Query, SearchOptions};
pub use tree::{Change, Child, Txn};

/// Row id of an entry. Stable across renames of the entry or its ancestors.
pub type Id = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    File = 0,
    Dir = 1,
    Symlink = 2,
    Other = 3,
}

impl Kind {
    pub fn from_i64(v: i64) -> Kind {
        match v {
            0 => Kind::File,
            1 => Kind::Dir,
            2 => Kind::Symlink,
            _ => Kind::Other,
        }
    }

    pub fn from_file_type(ft: std::fs::FileType) -> Kind {
        if ft.is_dir() {
            Kind::Dir
        } else if ft.is_file() {
            Kind::File
        } else if ft.is_symlink() {
            Kind::Symlink
        } else {
            Kind::Other
        }
    }
}

/// Change-detection metadata of an entry, as returned by `lstat`.
///
/// For directories the stored `mtime_ns` has a special meaning: it is the
/// directory mtime at the last *completed reconciliation of its listing*
/// (0 = never reconciled). [`Txn::upsert`] never changes it for directories;
/// only [`Txn::mark_dir_reconciled`] does. `size` is always 0 for directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub kind: Kind,
    pub size: u64,
    pub mtime_ns: i64,
    pub ino: u64,
}

impl Meta {
    pub fn from_metadata(m: &std::fs::Metadata) -> Meta {
        use std::os::unix::fs::MetadataExt;
        let kind = Kind::from_file_type(m.file_type());
        Meta {
            kind,
            size: if kind == Kind::Dir { 0 } else { m.size() },
            mtime_ns: m.mtime().saturating_mul(1_000_000_000).saturating_add(m.mtime_nsec()),
            ino: m.ino(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// The database exists but is not usable by this version.
    Incompatible(String),
}

impl Error {
    /// True when the database file itself is damaged; the caller should
    /// reopen with recovery ([`Writer::open`] moves it aside and recreates).
    pub fn is_corrupt(&self) -> bool {
        match self {
            Error::Sqlite(rusqlite::Error::SqliteFailure(e, _)) => {
                matches!(e.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
            }
            Error::Incompatible(_) => true,
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "index database error: {e}"),
            Error::Io(e) => write!(f, "index I/O error: {e}"),
            Error::Incompatible(s) => write!(f, "incompatible index: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The single read-write handle to an index database.
pub struct Writer {
    conn: rusqlite::Connection,
    path: PathBuf,
}

impl Writer {
    /// Opens (creating if needed) the index at `path`.
    ///
    /// A damaged, foreign or incompatible database is renamed to
    /// `<path>.broken` and a fresh one is created; the outcome says so, so
    /// the caller knows a full scan is required. After an unclean shutdown a
    /// `PRAGMA quick_check` runs first (cost grows with database size).
    pub fn open(path: &Path) -> Result<(Writer, OpenOutcome)> {
        let (conn, outcome) = schema::open_writer(path)?;
        Ok((Writer { conn, path: path.to_path_buf() }, outcome))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Starts a write transaction. Dropping the [`Txn`] without
    /// [`Txn::commit`] rolls back.
    pub fn transaction(&mut self) -> Result<Txn<'_>> {
        Txn::begin(&mut self.conn)
    }

    /// Invalidates directory listings atomically when the indexing policy changes.
    /// Unvisited listings remain invalid across an interrupted reconciliation.
    pub fn apply_policy(&mut self, policy: &str) -> Result<()> {
        use rusqlite::OptionalExtension;
        let tx = self.conn.transaction()?;
        let previous: Option<String> =
            tx.query_row("SELECT value FROM meta WHERE key='indexing_policy'", [], |r| r.get(0)).optional()?;
        if previous.as_deref() != Some(policy) {
            tx.execute("UPDATE entries SET mtime=0 WHERE kind=1", [])?;
            tx.execute(
                "INSERT INTO meta(key, value) VALUES('indexing_policy', ?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [policy],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records a clean shutdown and truncates the WAL. Call before exit.
    pub fn close(self) -> Result<()> {
        schema::mark_clean(&self.conn, true)?;
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        self.conn.close().map_err(|(_, e)| Error::Sqlite(e))
    }

    /// Checkpoints the WAL into the database and truncates it to zero
    /// bytes. Waits (busy timeout) for active readers; call when a pass
    /// ends, not in the middle of work.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub fn stats(&self) -> Result<Stats> {
        stats(&self.conn, &self.path)
    }

    /// Frees the connection's page cache and other releasable memory; call
    /// when going idle. The cache refills on demand.
    pub fn release_memory(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA shrink_memory;")?;
        Ok(())
    }
}

/// A read-only handle; cheap to open, safe to use concurrently with the
/// writer.
pub struct Reader {
    conn: rusqlite::Connection,
    path: PathBuf,
}

impl Reader {
    /// Opens an existing index read-only. Fails if it does not exist or has
    /// an incompatible schema version.
    pub fn open(path: &Path) -> Result<Reader> {
        let conn = schema::open_reader(path)?;
        Ok(Reader { conn, path: path.to_path_buf() })
    }

    /// Ranked filename/path search; see [`Query`] for the syntax.
    pub fn search(&self, query: &Query, opts: &SearchOptions) -> Result<Vec<Hit>> {
        let tx = self.conn.unchecked_transaction()?;
        let hits = search::run(&tx, query, opts)?;
        tx.commit()?;
        Ok(hits)
    }

    /// Absolute paths of the indexed roots. Cheap (index lookup on
    /// `parent IS NULL`), unlike [`Reader::stats`].
    pub fn roots(&self) -> Result<Vec<PathBuf>> {
        Ok(tree::roots(&self.conn)?.into_iter().map(|(_, p)| p).collect())
    }

    /// Resolves an absolute path to its entry, if indexed.
    pub fn lookup(&self, path: &Path) -> Result<Option<(Id, Meta)>> {
        tree::lookup_path(&self.conn, path)
    }

    pub fn stats(&self) -> Result<Stats> {
        stats(&self.conn, &self.path)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub other: u64,
    pub roots: Vec<PathBuf>,
    /// Main database file plus WAL, in bytes.
    pub disk_bytes: u64,
}

fn stats(conn: &rusqlite::Connection, path: &Path) -> Result<Stats> {
    let mut s = Stats::default();
    let mut st = conn.prepare_cached("SELECT kind, count(*) FROM entries GROUP BY kind")?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let n: i64 = r.get(1)?;
        match Kind::from_i64(r.get(0)?) {
            Kind::File => s.files += n as u64,
            Kind::Dir => s.dirs += n as u64,
            _ => s.other += n as u64,
        }
    }
    s.roots = tree::roots(conn)?.into_iter().map(|(_, p)| p).collect();
    for suffix in ["", "-wal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        if let Ok(m) = std::fs::metadata(&p) {
            s.disk_bytes += m.len();
        }
    }
    Ok(s)
}
