// SPDX-License-Identifier: GPL-3.0-or-later
//! Scanner exclusion rules; shared configuration types remain re-exported here.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub use ferroseek_config::{Config, ConfigError, DEFAULT_EXCLUDE_NAMES, Paths, expand, normalize};

/// Compiled exclusion rules, applied identically by the scanner and the
/// watcher.
#[derive(Clone, Debug)]
pub struct Rules {
    index_hidden: bool,
    exact: Vec<Vec<u8>>,
    globs: Vec<Vec<u8>>,
    paths: Vec<PathBuf>,
    pub respect_cachedir_tag: bool,
    pub stay_on_filesystem: bool,
    pub max_depth: u32,
}

impl Rules {
    pub(crate) fn has_exclusions_below(&self, dir: &Path) -> bool {
        self.paths.iter().any(|p| p.starts_with(dir))
    }

    /// `extra_paths` are always excluded too (FerroSeek's own data dir).
    pub fn new(cfg: &Config, extra_paths: &[PathBuf]) -> Rules {
        let (globs, exact): (Vec<_>, Vec<_>) = cfg
            .exclude_names
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .partition(|s| s.contains(&b'*') || s.contains(&b'?'));
        let mut paths: Vec<PathBuf> = cfg.exclude_paths.iter().map(|p| normalize(&expand(p))).collect();
        paths.extend(extra_paths.iter().cloned());
        Rules {
            index_hidden: cfg.index_hidden,
            exact,
            globs,
            paths,
            respect_cachedir_tag: cfg.respect_cachedir_tag,
            stay_on_filesystem: cfg.stay_on_filesystem,
            max_depth: cfg.max_depth,
        }
    }

    /// Whether the entry `name` inside directory `dir` must not be indexed.
    pub fn excluded(&self, dir: &Path, name: &[u8]) -> bool {
        if name.is_empty() || name == b"." || name == b".." {
            return true;
        }
        if !self.index_hidden && name[0] == b'.' {
            return true;
        }
        if self.exact.iter().any(|e| e == name) || self.globs.iter().any(|g| glob_match(g, name)) {
            return true;
        }
        if !self.paths.is_empty() {
            let full = dir.join(OsStr::from_bytes(name));
            if self.paths.iter().any(|p| full.starts_with(p)) {
                return true;
            }
        }
        false
    }
}

/// `*` (any run, including empty) and `?` (one byte) wildcard match over
/// raw bytes. Iterative with single-star backtracking: O(len(p)·len(s)).
pub fn glob_match(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        // The wildcard check must come first: a literal '*' in the name
        // would otherwise consume the pattern's '*' as a plain byte.
        if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, si));
            pi += 1;
        } else if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Cache Directory Tagging spec signature.
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// True if `dir` contains a regular, non-symlink `CACHEDIR.TAG` with a valid
/// signature (reads at most 43 bytes).
pub fn has_cachedir_tag(dir: &Path) -> bool {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // Nonblocking open also protects against a regular file being replaced
    // by a FIFO before open. Check the opened descriptor before reading.
    let Ok(mut f) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY)
        .open(dir.join("CACHEDIR.TAG"))
    else {
        return false;
    };
    if !f.metadata().is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut buf = [0u8; CACHEDIR_SIGNATURE.len()];
    f.read_exact(&mut buf).is_ok() && buf == CACHEDIR_SIGNATURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match(b"*.o", b"main.o"));
        assert!(!glob_match(b"*.o", b"main.oo"));
        assert!(glob_match(b"*~", b"file.txt~"));
        assert!(glob_match(b".#*", b".#lock"));
        assert!(glob_match(b"#*#", b"#autosave#"));
        assert!(glob_match(b"a?c", b"abc"));
        assert!(!glob_match(b"a?c", b"ac"));
        assert!(glob_match(b"*", b""));
        assert!(glob_match(b"*a*b*", b"xxaxxbxx"));
        assert!(!glob_match(b"*a*b", b"xxaxxbxx"));
        assert!(glob_match(b"*", b"*\0"), "regression: literal star in name");
        assert!(glob_match(b"*.o", b"*.o"));
    }

    proptest::proptest! {
        #[test]
        fn glob_star_matches_everything(s in proptest::collection::vec(proptest::num::u8::ANY, 0..64)) {
            proptest::prop_assert!(glob_match(b"*", &s));
            let literal: Vec<u8> = s.iter().map(|b| if *b == b'*' || *b == b'?' { b'?' } else { *b }).collect();
            let ok = glob_match(&literal, &s);
            proptest::prop_assert!(ok);
        }
    }

    #[test]
    fn rules_hidden_names_paths() {
        let mut c = Config { exclude_paths: vec!["/home/u/private".into()], ..Default::default() };
        let r = Rules::new(&c, &[PathBuf::from("/home/u/.local/share/ferroseek")]);
        let d = Path::new("/home/u");
        assert!(r.excluded(d, b".bashrc"));
        assert!(r.excluded(d, b"node_modules"));
        assert!(r.excluded(d, b"foo.o"));
        assert!(r.excluded(d, b"private"));
        assert!(!r.excluded(d, b"privateer"));
        assert!(!r.excluded(d, b"notes.md"));
        c.index_hidden = true;
        let r = Rules::new(&c, &[]);
        assert!(!r.excluded(d, b".bashrc"));
        assert!(r.excluded(d, b".git"), ".git is excluded by name even with hidden files on");
    }

    #[test]
    fn cachedir_tag_must_be_a_regular_file_without_following_symlinks() {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, symlink};

        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("signature");
        fs::write(&target, CACHEDIR_SIGNATURE).unwrap();
        let tag = t.path().join("CACHEDIR.TAG");
        symlink(&target, &tag).unwrap();
        assert!(!has_cachedir_tag(t.path()), "symlinks must not exclude the directory");
        fs::remove_file(&tag).unwrap();
        fs::create_dir(&tag).unwrap();
        assert!(!has_cachedir_tag(t.path()));
        fs::remove_dir(&tag).unwrap();
        let c_path = std::ffi::CString::new(tag.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a valid NUL-terminated path in this test's temp directory.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let mut pipe = fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(&tag).unwrap();
        pipe.write_all(CACHEDIR_SIGNATURE).unwrap();
        assert!(!has_cachedir_tag(t.path()), "a FIFO containing a signature is not a cache tag");
    }

    #[test]
    fn cachedir_tag() {
        let t = tempfile::tempdir().unwrap();
        assert!(!has_cachedir_tag(t.path()));
        fs::write(t.path().join("CACHEDIR.TAG"), b"Signature: wrong").unwrap();
        assert!(!has_cachedir_tag(t.path()));
        fs::write(t.path().join("CACHEDIR.TAG"), b"Signature: 8a477f597d28d172789f06886806bc55\n# x").unwrap();
        assert!(has_cachedir_tag(t.path()));
    }
}
