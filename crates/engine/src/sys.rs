// SPDX-License-Identifier: GPL-3.0-or-later
//! Small Linux-specific helpers: mount table, thread priorities, a wakeup
//! pipe, and a single-instance lock.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Mount points from `/proc/self/mountinfo`.
#[derive(Clone, Debug, Default)]
pub struct Mounts {
    points: HashSet<PathBuf>,
}

impl Mounts {
    pub fn load() -> Mounts {
        fs::read("/proc/self/mountinfo").map(|b| Mounts::parse(&b)).unwrap_or_default()
    }

    /// Field 5 of each mountinfo line is the mount point, with space, tab,
    /// newline and backslash octal-escaped (`\040` etc.).
    pub fn parse(b: &[u8]) -> Mounts {
        let mut points = HashSet::new();
        for line in b.split(|c| *c == b'\n') {
            if let Some(mp) = line.split(|c| *c == b' ').nth(4) {
                points.insert(PathBuf::from(OsStr::from_bytes(&unescape(mp))));
            }
        }
        Mounts { points }
    }

    pub fn is_mount_point(&self, p: &Path) -> bool {
        self.points.contains(p)
    }
}

fn unescape(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' && i + 3 < s.len() && s[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((s[i + 1] - b'0') * 64 + (s[i + 2] - b'0') * 8 + (s[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// Lowers the *calling thread's* CPU (nice 19) and I/O (idle class)
/// priority. Linux applies both per thread when given a TID, so query
/// threads keep normal priority. Failures are logged and ignored.
pub fn lower_current_thread_priority() {
    // SAFETY: plain syscalls with integer arguments.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        if libc::setpriority(libc::PRIO_PROCESS, tid, 19) != 0 {
            log::debug!("setpriority failed: {}", io::Error::last_os_error());
        }
        const IOPRIO_CLASS_IDLE: libc::c_long = 3;
        const IOPRIO_CLASS_SHIFT: libc::c_long = 13;
        const IOPRIO_WHO_PROCESS: libc::c_long = 1;
        if libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, tid as libc::c_long, IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT) != 0 {
            log::debug!("ioprio_set failed: {}", io::Error::last_os_error());
        }
    }
}

/// Returns freed heap memory to the kernel (glibc keeps it otherwise).
pub fn trim_heap() {
    #[cfg(target_env = "gnu")]
    // SAFETY: malloc_trim has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// A self-pipe used to wake a thread blocked in `poll`.
pub struct WakePipe {
    pub read: OwnedFd,
    pub write: OwnedFd,
}

impl WakePipe {
    pub fn new() -> io::Result<WakePipe> {
        let mut fds = [0; 2];
        // SAFETY: pipe2 writes two fds into the array on success.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both fds are fresh and owned by us.
        Ok(unsafe { WakePipe { read: OwnedFd::from_raw_fd(fds[0]), write: OwnedFd::from_raw_fd(fds[1]) } })
    }

    pub fn wake(&self) {
        // SAFETY: writing one byte from a valid buffer to our own fd.
        unsafe { libc::write(self.write.as_raw_fd(), b"x".as_ptr().cast(), 1) };
    }
}

/// Blocks until `a` or `b` is readable. Returns (a_ready, b_ready).
pub fn poll2(a: i32, b: i32) -> io::Result<(bool, bool)> {
    let mut fds = [
        libc::pollfd { fd: a, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: b, events: libc::POLLIN, revents: 0 },
    ];
    loop {
        // SAFETY: fds is a valid array of two pollfd.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if r >= 0 {
            return Ok((fds[0].revents != 0, fds[1].revents != 0));
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// An exclusive `flock` on a file, held for the life of the value. Ensures
/// one writer (daemon or one-shot CLI indexer) per index.
pub struct InstanceLock {
    _file: fs::File,
}

impl InstanceLock {
    /// Returns `Ok(None)` if another process holds the lock.
    pub fn try_acquire(path: &Path) -> io::Result<Option<InstanceLock>> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = fs::OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(path)?;
        // SAFETY: flock on a valid fd.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(None);
            }
            return Err(e);
        }
        Ok(Some(InstanceLock { _file: f }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_parsing_with_escapes() {
        let s = b"22 1 0:21 / / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw\n\
                  30 22 0:5 / /proc rw - proc proc rw\n\
                  40 22 0:50 / /run/media/u/My\\040Disk rw - vfat /dev/sdb1 rw\n";
        let m = Mounts::parse(s);
        assert!(m.is_mount_point(Path::new("/proc")));
        assert!(m.is_mount_point(Path::new("/run/media/u/My Disk")));
        assert!(!m.is_mount_point(Path::new("/home")));
    }

    #[test]
    fn real_mountinfo_has_root() {
        assert!(Mounts::load().is_mount_point(Path::new("/")));
    }

    #[test]
    fn instance_lock_is_exclusive() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("l");
        let a = InstanceLock::try_acquire(&p).unwrap();
        assert!(a.is_some());
        // flock is per open file description, so a second open conflicts
        // even within one process.
        assert!(InstanceLock::try_acquire(&p).unwrap().is_none());
        drop(a);
        assert!(InstanceLock::try_acquire(&p).unwrap().is_some());
    }
}
