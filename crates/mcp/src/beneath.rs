// SPDX-License-Identifier: GPL-3.0-or-later
//! Metadata of a path *beneath* an allowed directory, without following
//! symlinks at any level.
//!
//! `lstat("/allowed/a/b/file")` does not follow the final component but does
//! follow `a` and `b`. If `a` was replaced by a symlink to an unauthorized
//! directory after indexing, a plain `lstat` would report metadata from
//! outside the allowlist. Here every intermediate component is opened with
//! `openat(O_PATH | O_DIRECTORY | O_NOFOLLOW)` relative to the previous one
//! and must be a real directory; the final component is `fstatat`ed with
//! `AT_SYMLINK_NOFOLLOW`. That is race-free: each step is anchored to an
//! already-verified directory fd.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use ferroseek_index::{Kind, Meta};

fn cstr(b: &[u8]) -> io::Result<CString> {
    CString::new(b).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn check(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

// `stat` field widths differ across Linux targets (time_t/ino_t are 32-bit
// on some); the `From` conversions are no-ops only on 64-bit.
#[allow(clippy::useless_conversion)]
fn meta_of(st: &libc::stat) -> Meta {
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => Kind::Dir,
        libc::S_IFREG => Kind::File,
        libc::S_IFLNK => Kind::Symlink,
        _ => Kind::Other,
    };
    Meta {
        kind,
        size: if kind == Kind::Dir { 0 } else { st.st_size as u64 },
        mtime_ns: i64::from(st.st_mtime).saturating_mul(1_000_000_000).saturating_add(i64::from(st.st_mtime_nsec)),
        ino: u64::from(st.st_ino),
    }
}

fn is_dir(fd: &OwnedFd) -> io::Result<bool> {
    // SAFETY: fstat on a valid fd into a zeroed stat buffer.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstat(fd.as_raw_fd(), &mut st) })?;
    Ok(st.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

/// Metadata of `base/rel`. `base` itself is opened normally (it is the
/// user's explicit choice); nothing below it is followed. A symlinked or
/// non-directory intermediate component yields `ENOTDIR`/`ELOOP`; a missing
/// one `ENOENT`. Callers treat all of these as "the indexed path no longer
/// refers to an entry inside the allowed directory".
pub fn stat_beneath(base: &Path, rel: &Path) -> io::Result<Meta> {
    let flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC;
    // SAFETY: open with a NUL-terminated path; the fd is owned below.
    let mut dir =
        unsafe { OwnedFd::from_raw_fd(check(libc::open(cstr(base.as_os_str().as_bytes())?.as_ptr(), flags))?) };
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => parts.push(n.as_bytes()),
            _ => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        }
    }
    let Some((last, ancestors)) = parts.split_last() else {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fstat on our own fd.
        check(unsafe { libc::fstat(dir.as_raw_fd(), &mut st) })?;
        return Ok(meta_of(&st));
    };
    for name in ancestors {
        let c = cstr(name)?;
        // SAFETY: openat relative to a valid directory fd.
        let fd = check(unsafe { libc::openat(dir.as_raw_fd(), c.as_ptr(), flags | libc::O_NOFOLLOW) })?;
        // SAFETY: fd is fresh and owned from here on.
        dir = unsafe { OwnedFd::from_raw_fd(fd) };
        // O_PATH|O_NOFOLLOW can open a symlink itself; refuse anything that
        // is not a real directory.
        if !is_dir(&dir)? {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
    }
    let c = cstr(last)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstatat relative to a verified directory fd.
    check(unsafe { libc::fstatat(dir.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
    Ok(meta_of(&st))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn plain_paths_and_final_symlinks() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("a/b")).unwrap();
        std::fs::write(t.path().join("a/b/f.txt"), b"12345").unwrap();
        symlink("/etc/passwd", t.path().join("a/link")).unwrap();
        let m = stat_beneath(t.path(), Path::new("a/b/f.txt")).unwrap();
        assert_eq!((m.kind, m.size), (Kind::File, 5));
        assert_eq!(stat_beneath(t.path(), Path::new("a/link")).unwrap().kind, Kind::Symlink, "final link not followed");
        assert_eq!(stat_beneath(t.path(), Path::new("")).unwrap().kind, Kind::Dir);
        assert_eq!(stat_beneath(t.path(), Path::new("a/missing")).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn symlinked_ancestor_is_never_followed() {
        let t = tempfile::tempdir().unwrap();
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("record.txt"), b"secret-size-12345").unwrap();
        let allowed = t.path().join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        symlink(&outside, allowed.join("sub")).unwrap();
        // A plain lstat leaks the outside file's metadata …
        assert_eq!(std::fs::symlink_metadata(allowed.join("sub/record.txt")).unwrap().len(), 17);
        // … stat_beneath refuses to traverse the link.
        let e = stat_beneath(&allowed, Path::new("sub/record.txt")).unwrap_err();
        assert!(matches!(e.raw_os_error(), Some(libc::ENOTDIR) | Some(libc::ELOOP)), "{e}");
        assert!(stat_beneath(&allowed, Path::new("../outside/record.txt")).is_err(), "no parent components");
    }
}
