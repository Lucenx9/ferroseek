// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal inotify binding over `libc`.
//!
//! Written directly instead of using the `inotify` crate: the surface is
//! four syscalls. Doing it ourselves means no panic on unknown mask bits and
//! plain `i32` watch descriptors (removal does not need a stored handle). It
//! also lets us poll the fd together with a wakeup pipe.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

pub const IN_MODIFY: u32 = libc::IN_MODIFY;
pub const IN_ATTRIB: u32 = libc::IN_ATTRIB;
pub const IN_CLOSE_WRITE: u32 = libc::IN_CLOSE_WRITE;
pub const IN_MOVED_FROM: u32 = libc::IN_MOVED_FROM;
pub const IN_MOVED_TO: u32 = libc::IN_MOVED_TO;
pub const IN_CREATE: u32 = libc::IN_CREATE;
pub const IN_DELETE: u32 = libc::IN_DELETE;
pub const IN_DELETE_SELF: u32 = libc::IN_DELETE_SELF;
pub const IN_MOVE_SELF: u32 = libc::IN_MOVE_SELF;
pub const IN_UNMOUNT: u32 = libc::IN_UNMOUNT;
pub const IN_Q_OVERFLOW: u32 = libc::IN_Q_OVERFLOW;
pub const IN_IGNORED: u32 = libc::IN_IGNORED;
pub const IN_ISDIR: u32 = libc::IN_ISDIR;

/// Events FerroSeek subscribes to on every indexed directory. `IN_MODIFY`
/// is deliberately absent (one event per write(2)); `IN_CLOSE_WRITE`
/// covers completed writes and `IN_ATTRIB` covers `touch`/`chmod`.
pub const DIR_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW
    | libc::IN_EXCL_UNLINK;

/// One decoded inotify event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEvent {
    pub wd: i32,
    pub mask: u32,
    pub cookie: u32,
    /// Empty for events about the watched directory itself.
    pub name: Vec<u8>,
}

/// An inotify instance. Cloning shares the fd: one clone blocks reading
/// while another adds and removes watches.
#[derive(Clone)]
pub struct Inotify {
    fd: Arc<OwnedFd>,
}

impl Inotify {
    pub fn new() -> io::Result<Inotify> {
        // SAFETY: plain syscall; returns a new fd or -1.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is fresh and owned by us.
        Ok(Inotify { fd: Arc::new(unsafe { OwnedFd::from_raw_fd(fd) }) })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Adds (or updates) a watch. Watching the same inode again returns the
    /// same wd. `ENOSPC` means `fs.inotify.max_user_watches` is exhausted.
    pub fn add_watch(&self, path: &Path, mask: u32) -> io::Result<i32> {
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: valid fd and NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(self.raw_fd(), c.as_ptr(), mask) };
        if wd < 0 { Err(io::Error::last_os_error()) } else { Ok(wd) }
    }

    pub fn rm_watch(&self, wd: i32) -> io::Result<()> {
        // SAFETY: plain syscall.
        if unsafe { libc::inotify_rm_watch(self.raw_fd(), wd) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Reads all currently queued events without blocking (the fd is
    /// non-blocking). Appends to `out`; returns how many were read.
    pub fn read_available(&self, buf: &mut [u8], out: &mut Vec<RawEvent>) -> io::Result<usize> {
        let mut n_total = 0;
        loop {
            // SAFETY: reading into a valid, writable buffer.
            let n = unsafe { libc::read(self.raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                return match e.kind() {
                    io::ErrorKind::WouldBlock => Ok(n_total),
                    io::ErrorKind::Interrupted => continue,
                    _ => Err(e),
                };
            }
            if n == 0 {
                return Ok(n_total);
            }
            n_total += parse(&buf[..n as usize], out);
        }
    }
}

const HDR: usize = std::mem::size_of::<libc::inotify_event>();

/// Decodes a buffer of packed `inotify_event`s. Truncated trailing data is
/// ignored (the kernel never splits events across reads).
pub fn parse(buf: &[u8], out: &mut Vec<RawEvent>) -> usize {
    let mut off = 0;
    let mut n = 0;
    while off + HDR <= buf.len() {
        // SAFETY: bounds checked above; read_unaligned tolerates any
        // alignment of the byte buffer.
        let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[off..].as_ptr().cast()) };
        let len = ev.len as usize;
        let name_start = off + HDR;
        let Some(name_bytes) = buf.get(name_start..name_start + len) else { break };
        let end = name_bytes.iter().position(|b| *b == 0).unwrap_or(len);
        out.push(RawEvent { wd: ev.wd, mask: ev.mask, cookie: ev.cookie, name: name_bytes[..end].to_vec() });
        n += 1;
        off = name_start + len;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_rename_delete_and_parse() {
        let t = tempfile::tempdir().unwrap();
        let ino = Inotify::new().unwrap();
        let wd = ino.add_watch(t.path(), DIR_MASK).unwrap();
        assert_eq!(ino.add_watch(t.path(), DIR_MASK).unwrap(), wd, "same inode, same wd");
        std::fs::write(t.path().join("a"), b"x").unwrap();
        std::fs::rename(t.path().join("a"), t.path().join("b")).unwrap();
        std::fs::remove_file(t.path().join("b")).unwrap();
        let mut buf = vec![0u8; 4096];
        let mut evs = Vec::new();
        ino.read_available(&mut buf, &mut evs).unwrap();
        let kinds: Vec<(u32, &[u8])> = evs.iter().map(|e| (e.mask & !IN_ISDIR, e.name.as_slice())).collect();
        assert_eq!(
            kinds,
            vec![
                (IN_CREATE, &b"a"[..]),
                (IN_CLOSE_WRITE, b"a"),
                (IN_MOVED_FROM, b"a"),
                (IN_MOVED_TO, b"b"),
                (IN_DELETE, b"b")
            ]
        );
        assert_eq!(evs[2].cookie, evs[3].cookie);
        assert_ne!(evs[2].cookie, 0);
        ino.rm_watch(wd).unwrap();
        evs.clear();
        ino.read_available(&mut buf, &mut evs).unwrap();
        assert!(evs.iter().any(|e| e.mask & IN_IGNORED != 0));
    }

    #[test]
    fn parse_tolerates_truncation() {
        let mut out = Vec::new();
        assert_eq!(parse(&[0u8; 5], &mut out), 0);
    }
}
