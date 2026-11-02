// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded plain-text reads from an indexed inode, without following symlinks.
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

const MAX_BYTES: usize = 64 * 1024;
const MAX_CHARS: usize = 16_384;
const MAX_LINES: usize = 200;

pub enum Error {
    Unavailable,
    Unsupported(&'static str),
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn checked_fd(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful open/openat returns a fresh descriptor owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn pin(path: &Path, inode: u64) -> io::Result<File> {
    let mut parts = path.components();
    if parts.next() != Some(Component::RootDir) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let names: Vec<_> = parts.collect();
    if names.is_empty() || names.iter().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let flags = libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: NUL-terminated root path and flags without a creation mode.
    let mut fd = checked_fd(unsafe { libc::open(c"/".as_ptr(), flags | libc::O_DIRECTORY) })?;
    for (index, part) in names.iter().enumerate() {
        let name = CString::new(part.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let flags = flags | if index + 1 < names.len() { libc::O_DIRECTORY } else { 0 };
        // SAFETY: the previous directory fd and NUL-terminated component are valid.
        // O_PATH pins even special files without opening them for I/O.
        fd = checked_fd(unsafe { libc::openat(fd.as_raw_fd(), name.as_ptr(), flags) })?;
    }
    let file = File::from(fd);
    let meta = file.metadata()?;
    if !meta.is_file() || meta.ino() != inode {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

pub fn read(path: &Path, inode: u64, mime: &str) -> Result<(String, bool), Error> {
    let pinned = pin(path, inode).map_err(|_| Error::Unavailable)?;
    if !mime.starts_with("text/")
        && !matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/toml"
                | "application/yaml"
                | "application/x-yaml"
        )
    {
        return Err(Error::Unsupported("Text preview is not available for this file type."));
    }
    read_pinned(&pinned)
}

fn read_pinned(pinned: &File) -> Result<(String, bool), Error> {
    let before = pinned.metadata()?;
    // Reopen our own pinned regular inode, never the caller's path again. The
    // O_PATH fd stays alive throughout; FIFO/device open side effects are avoided.
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.ino() != before.ino() || opened.dev() != before.dev() {
        return Err(Error::Unavailable);
    }
    let mut bytes = Vec::with_capacity(MAX_BYTES + 1);
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    let after = pinned.metadata()?;
    if before.len() != after.len() || before.mtime() != after.mtime() || before.mtime_nsec() != after.mtime_nsec() {
        return Err(Error::Unavailable);
    }
    if bytes.iter().any(|byte| *byte < 32 && !matches!(*byte, b'\n' | b'\r' | b'\t')) {
        return Err(Error::Unsupported("The inspected text contains binary control characters."));
    }
    let mut truncated = bytes.len() > MAX_BYTES;
    bytes.truncate(MAX_BYTES);
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| Error::Unavailable)?
        }
        Err(_) => return Err(Error::Unsupported("The inspected text is not valid UTF-8.")),
    };
    let mut text = text.replace("\r\n", "\n").replace('\r', "\n");
    let chars_end = text.char_indices().nth(MAX_CHARS).map_or(text.len(), |(index, _)| index);
    let lines_end = text.match_indices('\n').nth(MAX_LINES - 1).map_or(text.len(), |(index, _)| index);
    let end = chars_end.min(lines_end);
    truncated |= end < text.len();
    text.truncate(end);
    Ok((text, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_pinned_file_does_not_follow_a_replaced_path() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original.txt");
        std::fs::write(&path, "original contents").unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        let pinned = pin(&path, inode).unwrap();
        std::fs::rename(&path, root.path().join("held.txt")).unwrap();
        let outside = root.path().join("outside.txt");
        std::fs::write(&outside, "replacement contents").unwrap();
        symlink(outside, &path).unwrap();
        assert!(matches!(read_pinned(&pinned), Ok((text, false)) if text == "original contents"));
        assert!(pin(&path, inode).is_err());
    }
}
