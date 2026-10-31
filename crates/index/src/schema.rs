// SPDX-License-Identifier: GPL-3.0-or-later
//! Schema creation, versioning and recovery.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::{Error, Result};

/// Bump on any incompatible schema change. Older/newer databases are moved
/// aside and rebuilt (the index is a cache of the filesystem).
pub const SCHEMA_VERSION: i64 = 1;

/// `PRAGMA application_id`, "FSK1": identifies FerroSeek databases.
const APPLICATION_ID: i64 = 0x4653_4B31;

const SCHEMA: &str = "
CREATE TABLE entries(
    id     INTEGER PRIMARY KEY,
    parent INTEGER,
    name   BLOB    NOT NULL,
    key    TEXT    NOT NULL,
    kind   INTEGER NOT NULL,
    size   INTEGER NOT NULL,
    mtime  INTEGER NOT NULL,
    ino    INTEGER NOT NULL
);
CREATE UNIQUE INDEX entries_parent_name ON entries(parent, name);
CREATE INDEX entries_key ON entries(key);
CREATE VIRTUAL TABLE names USING fts5(
    key, content='', contentless_delete=1, tokenize='trigram'
);
CREATE TABLE meta(key TEXT PRIMARY KEY, value) WITHOUT ROWID;
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    /// Existing, healthy index.
    Existing,
    /// No index existed; a new empty one was created.
    Created,
    /// The previous index was unusable and was moved to `moved_to`; a new
    /// empty one was created.
    Recovered { reason: String, moved_to: PathBuf },
}

fn configure_writer(conn: &Connection) -> Result<()> {
    // WAL + NORMAL: crash-safe and never corrupt; power loss may drop the
    // last commits, which startup reconciliation repairs.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA cache_size=-4096;
         PRAGMA journal_size_limit=67108864;
         PRAGMA foreign_keys=OFF;",
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

fn is_empty_db(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    Ok(n == 0)
}

fn check_version(conn: &Connection) -> Result<()> {
    let app: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if app != APPLICATION_ID {
        return Err(Error::Incompatible(format!("not a FerroSeek index (application_id {app:#x})")));
    }
    if ver != SCHEMA_VERSION {
        return Err(Error::Incompatible(format!(
            "schema version {ver}, this build uses {SCHEMA_VERSION}"
        )));
    }
    Ok(())
}

fn create(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "BEGIN;
         {SCHEMA}
         PRAGMA application_id={APPLICATION_ID};
         PRAGMA user_version={SCHEMA_VERSION};
         COMMIT;"
    ))?;
    Ok(())
}

pub(crate) fn mark_clean(conn: &Connection, clean: bool) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('clean_shutdown', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [clean as i64],
    )?;
    Ok(())
}

fn was_clean(conn: &Connection) -> Result<bool> {
    let v: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key='clean_shutdown'", [], |r| r.get(0))
        .ok();
    Ok(v == Some(1))
}

/// Opens and validates an existing or new database. Returns `Ok(None)` from
/// the inner attempt when it must be recovered.
fn try_open(path: &Path) -> Result<(Connection, OpenOutcome)> {
    let existed = path.exists();
    let conn = Connection::open(path)?;
    if !existed {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    configure_writer(&conn)?;
    if is_empty_db(&conn)? {
        create(&conn)?;
        mark_clean(&conn, false)?;
        return Ok((conn, OpenOutcome::Created));
    }
    check_version(&conn)?;
    if !was_clean(&conn)? {
        let res: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
        if res != "ok" {
            return Err(Error::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(format!("quick_check: {res}")),
            )));
        }
    }
    mark_clean(&conn, false)?;
    Ok((conn, OpenOutcome::Existing))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

pub(crate) fn open_writer(path: &Path) -> Result<(Connection, OpenOutcome)> {
    match try_open(path) {
        Ok(v) => Ok(v),
        Err(e) if e.is_corrupt() => {
            let reason = e.to_string();
            log::warn!("index at {} is unusable ({reason}); moving it aside and rebuilding", path.display());
            let moved_to = sidecar(path, ".broken");
            // Only our own cache files are touched: the database and its
            // WAL/SHM sidecars.
            fs::rename(path, &moved_to)?;
            for s in ["-wal", "-shm"] {
                let _ = fs::remove_file(sidecar(path, s));
            }
            let (conn, _) = try_open(path)?;
            Ok((conn, OpenOutcome::Recovered { reason, moved_to }))
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch("PRAGMA cache_size=-2048; PRAGMA query_only=1;")?;
    check_version(&conn)?;
    Ok(conn)
}
