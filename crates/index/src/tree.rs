// SPDX-License-Identifier: GPL-3.0-or-later
//! Tree mutations. All keep `entries` and the `names` FTS table in sync.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{Id, Kind, Meta, Result};

/// A stored child entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub id: Id,
    pub meta: Meta,
}

/// What [`Txn::upsert`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Inserted,
    Updated,
    Unchanged,
}

/// Lowercased, lossily-decoded name used for matching.
pub(crate) fn fold(name: &[u8]) -> String {
    String::from_utf8_lossy(name).to_lowercase()
}

fn row_meta(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Meta> {
    Ok(Meta {
        kind: Kind::from_i64(r.get(at)?),
        size: r.get::<_, i64>(at + 1)? as u64,
        mtime_ns: r.get(at + 2)?,
        ino: r.get::<_, i64>(at + 3)? as u64,
    })
}

/// A write transaction over the tree. Dropped without [`Txn::commit`] it
/// rolls back.
pub struct Txn<'c> {
    tx: Transaction<'c>,
    ops: usize,
}

impl<'c> Txn<'c> {
    pub(crate) fn begin(conn: &'c mut Connection) -> Result<Txn<'c>> {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS doomed(id INTEGER PRIMARY KEY);")?;
        Ok(Txn { tx, ops: 0 })
    }

    /// Number of row mutations so far; callers use it to size batches.
    pub fn ops(&self) -> usize {
        self.ops
    }

    pub fn commit(self) -> Result<()> {
        self.tx.commit()?;
        Ok(())
    }

    /// All roots as `(id, absolute path)`.
    pub fn roots(&self) -> Result<Vec<(Id, PathBuf)>> {
        roots(&self.tx)
    }

    /// Returns the root entry for `path`, creating it if needed.
    pub fn ensure_root(&mut self, path: &Path, meta: Meta) -> Result<Id> {
        let name = path.as_os_str().as_bytes();
        let existing: Option<Id> = self
            .tx
            .prepare_cached("SELECT id FROM entries WHERE parent IS NULL AND name = ?1")?
            .query_row([name], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        self.insert(None, name, meta)
    }

    pub fn child(&self, parent: Id, name: &[u8]) -> Result<Option<Child>> {
        Ok(self
            .tx
            .prepare_cached("SELECT id, kind, size, mtime, ino FROM entries WHERE parent = ?1 AND name = ?2")?
            .query_row(params![parent, name], |r| Ok(Child { id: r.get(0)?, meta: row_meta(r, 1)? }))
            .optional()?)
    }

    /// All children of `parent`, sorted by raw name bytes.
    pub fn children(&self, parent: Id) -> Result<Vec<(Vec<u8>, Child)>> {
        let mut st = self
            .tx
            .prepare_cached("SELECT name, id, kind, size, mtime, ino FROM entries WHERE parent = ?1 ORDER BY name")?;
        let rows = st.query_map([parent], |r| Ok((r.get(0)?, Child { id: r.get(1)?, meta: row_meta(r, 2)? })))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Child directories of `parent` as `(id, name)`.
    pub fn child_dirs(&self, parent: Id) -> Result<Vec<(Id, Vec<u8>)>> {
        let mut st = self.tx.prepare_cached("SELECT id, name FROM entries WHERE parent = ?1 AND kind = 1")?;
        let rows = st.query_map([parent], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn meta(&self, id: Id) -> Result<Option<Meta>> {
        Ok(self
            .tx
            .prepare_cached("SELECT kind, size, mtime, ino FROM entries WHERE id = ?1")?
            .query_row([id], |r| row_meta(r, 0))
            .optional()?)
    }

    /// Absolute path of `id`, or `None` if it no longer exists.
    pub fn path(&self, id: Id) -> Result<Option<PathBuf>> {
        path_of(&self.tx, id)
    }

    fn insert(&mut self, parent: Option<Id>, name: &[u8], meta: Meta) -> Result<Id> {
        let key = fold(name);
        // Directory mtime means "listing reconciled at"; a new dir has never
        // been reconciled.
        let mtime = if meta.kind == Kind::Dir { 0 } else { meta.mtime_ns };
        self.tx
            .prepare_cached(
                "INSERT INTO entries(parent, name, key, kind, size, mtime, ino) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?
            .execute(params![parent, name, key, meta.kind as i64, meta.size as i64, mtime, meta.ino as i64])?;
        let id = self.tx.last_insert_rowid();
        self.tx.prepare_cached("INSERT INTO names(rowid, key) VALUES(?1, ?2)")?.execute(params![id, key])?;
        self.ops += 1;
        Ok(id)
    }

    /// Inserts or updates `name` under `parent` to match `meta`.
    ///
    /// If an existing directory becomes a non-directory, its subtree is
    /// deleted. For directories the stored mtime is left alone (see
    /// [`Meta`]).
    pub fn upsert(&mut self, parent: Id, name: &[u8], meta: Meta) -> Result<(Id, Change)> {
        let Some(old) = self.child(parent, name)? else {
            return Ok((self.insert(Some(parent), name, meta)?, Change::Inserted));
        };
        let mut want = meta;
        if meta.kind == Kind::Dir && old.meta.kind == Kind::Dir {
            want.mtime_ns = old.meta.mtime_ns;
            if old.meta.ino != meta.ino {
                // Same name, different directory (replaced): its listing
                // must be reconciled from scratch.
                want.mtime_ns = 0;
            }
        }
        if want == old.meta {
            return Ok((old.id, Change::Unchanged));
        }
        if old.meta.kind == Kind::Dir && meta.kind != Kind::Dir {
            self.delete_children(old.id)?;
        }
        if old.meta.kind != Kind::Dir && meta.kind == Kind::Dir {
            want.mtime_ns = 0;
        }
        self.set_meta(old.id, want)?;
        Ok((old.id, Change::Updated))
    }

    fn set_meta(&mut self, id: Id, m: Meta) -> Result<()> {
        self.tx
            .prepare_cached("UPDATE entries SET kind=?2, size=?3, mtime=?4, ino=?5 WHERE id=?1")?
            .execute(params![id, m.kind as i64, m.size as i64, m.mtime_ns, m.ino as i64])?;
        self.ops += 1;
        Ok(())
    }

    /// Records that the listing of directory `id` was fully reconciled while
    /// the directory had mtime `mtime_ns` (and inode `ino`).
    pub fn mark_dir_reconciled(&mut self, id: Id, mtime_ns: i64, ino: u64) -> Result<()> {
        self.tx
            .prepare_cached("UPDATE entries SET mtime=?2, ino=?3 WHERE id=?1 AND kind=1")?
            .execute(params![id, mtime_ns, ino as i64])?;
        self.ops += 1;
        Ok(())
    }

    /// Ids of `id` and all directories below it (for dropping watches).
    pub fn subtree_dirs(&self, id: Id) -> Result<Vec<Id>> {
        let mut st = self.tx.prepare_cached(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1 UNION ALL
                 SELECT e.id FROM entries e JOIN sub ON e.parent = sub.id WHERE e.kind = 1)
             SELECT id FROM sub",
        )?;
        let rows = st.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn delete_children(&mut self, id: Id) -> Result<usize> {
        self.collect_doomed(id, false)?;
        self.delete_doomed()
    }

    /// Deletes `id` and its whole subtree. Returns the number of entries
    /// removed (0 if `id` did not exist).
    pub fn delete(&mut self, id: Id) -> Result<usize> {
        self.collect_doomed(id, true)?;
        self.delete_doomed()
    }

    fn collect_doomed(&mut self, id: Id, include_self: bool) -> Result<()> {
        self.tx
            .prepare_cached(
                "INSERT OR IGNORE INTO doomed(id)
             WITH RECURSIVE sub(id) AS (
                 SELECT id FROM entries WHERE id = ?1 UNION ALL
                 SELECT e.id FROM entries e JOIN sub ON e.parent = sub.id)
             SELECT id FROM sub WHERE ?2 OR id != ?1",
            )?
            .execute(params![id, include_self])?;
        Ok(())
    }

    fn delete_doomed(&mut self) -> Result<usize> {
        // FTS5 deletes must go row by row: a `rowid IN (subquery)` on a
        // contentless table would scan the whole index.
        let ids: Vec<Id> = {
            let mut st = self.tx.prepare_cached("SELECT id FROM doomed")?;
            st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        {
            let mut del = self.tx.prepare_cached("DELETE FROM names WHERE rowid = ?1")?;
            for id in &ids {
                del.execute([id])?;
            }
        }
        let n = self.tx.prepare_cached("DELETE FROM entries WHERE id IN (SELECT id FROM doomed)")?.execute([])?;
        self.tx.prepare_cached("DELETE FROM doomed")?.execute([])?;
        self.ops += n;
        Ok(n)
    }

    /// Moves `id` to `new_parent`/`new_name`. Whatever was stored at the
    /// destination (a replaced file or directory) is deleted first. The
    /// subtree moves with it at O(1) cost.
    pub fn rename(&mut self, id: Id, new_parent: Id, new_name: &[u8]) -> Result<()> {
        if let Some(dst) = self.child(new_parent, new_name)? {
            if dst.id == id {
                return Ok(());
            }
            self.delete(dst.id)?;
        }
        let key = fold(new_name);
        self.tx
            .prepare_cached("UPDATE entries SET parent=?2, name=?3, key=?4 WHERE id=?1")?
            .execute(params![id, new_parent, new_name, key])?;
        self.tx.prepare_cached("DELETE FROM names WHERE rowid = ?1")?.execute([id])?;
        self.tx.prepare_cached("INSERT INTO names(rowid, key) VALUES(?1, ?2)")?.execute(params![id, key])?;
        self.ops += 1;
        Ok(())
    }

    /// Resolves an absolute path to its entry, if indexed.
    pub fn lookup(&self, path: &Path) -> Result<Option<(Id, Meta)>> {
        lookup_path(&self.tx, path)
    }
}

pub(crate) fn roots(conn: &Connection) -> Result<Vec<(Id, PathBuf)>> {
    let mut st = conn.prepare_cached("SELECT id, name FROM entries WHERE parent IS NULL ORDER BY id")?;
    let rows = st.query_map([], |r| {
        let name: Vec<u8> = r.get(1)?;
        Ok((r.get(0)?, PathBuf::from(OsStr::from_bytes(&name))))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn path_of(conn: &Connection, mut id: Id) -> Result<Option<PathBuf>> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut st = conn.prepare_cached("SELECT parent, name FROM entries WHERE id = ?1")?;
    loop {
        let row: Option<(Option<Id>, Vec<u8>)> = st.query_row([id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((parent, name)) = row else { return Ok(None) };
        parts.push(name);
        match parent {
            Some(p) => id = p,
            None => break,
        }
        if parts.len() > 4096 {
            // A parent cycle can only come from a bug; refuse to loop.
            return Ok(None);
        }
    }
    let mut out = PathBuf::from(OsStr::from_bytes(&parts.pop().unwrap_or_default()));
    while let Some(p) = parts.pop() {
        out.push(OsStr::from_bytes(&p));
    }
    Ok(Some(out))
}

pub(crate) fn lookup_path(conn: &Connection, path: &Path) -> Result<Option<(Id, Meta)>> {
    // Longest root that is a prefix of `path`.
    let mut best: Option<(Id, PathBuf)> = None;
    for (id, root) in roots(conn)? {
        if path.starts_with(&root) && best.as_ref().is_none_or(|(_, b)| root.as_os_str().len() > b.as_os_str().len()) {
            best = Some((id, root));
        }
    }
    let Some((mut id, root)) = best else { return Ok(None) };
    let rest = path.strip_prefix(&root).unwrap_or(Path::new(""));
    let mut st = conn.prepare_cached("SELECT id FROM entries WHERE parent = ?1 AND name = ?2")?;
    for c in rest.components() {
        let Component::Normal(name) = c else { return Ok(None) };
        match st.query_row(params![id, name.as_bytes()], |r| r.get(0)).optional()? {
            Some(child) => id = child,
            None => return Ok(None),
        }
    }
    let meta = conn
        .prepare_cached("SELECT kind, size, mtime, ino FROM entries WHERE id = ?1")?
        .query_row([id], |r| row_meta(r, 0))
        .optional()?;
    Ok(meta.map(|m| (id, m)))
}
