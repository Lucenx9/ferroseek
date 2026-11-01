// SPDX-License-Identifier: GPL-3.0-or-later
//! `file://` URLs as produced by the daemon's runner (every byte outside
//! RFC 3986 unreserved and `/` percent-encoded).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Decodes a local `file:///…` URL. `None` for other hosts, malformed
/// escapes or embedded NULs.
pub fn path_from_file_url(u: &str) -> Option<PathBuf> {
    let rest = u.strip_prefix("file://")?;
    if !rest.starts_with('/') {
        return None;
    }
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_runner_urls() {
        assert_eq!(path_from_file_url("file:///t/a%20b%23c").unwrap(), PathBuf::from("/t/a b#c"));
        let raw = path_from_file_url("file:///t/bad%FF").unwrap();
        assert_eq!(raw.as_os_str().as_bytes(), b"/t/bad\xff");
        assert!(path_from_file_url("file://host/x").is_none());
        assert!(path_from_file_url("file:///x%2").is_none());
        assert!(path_from_file_url("file:///x%00").is_none());
        assert!(path_from_file_url("http:///x").is_none());
    }
}
