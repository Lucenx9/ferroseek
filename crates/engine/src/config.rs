// SPDX-License-Identifier: GPL-3.0-or-later
//! Configuration, XDG paths and exclusion rules.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where FerroSeek keeps its files. Everything honours the XDG base
/// directory spec; `FERROSEEK_CONFIG` and `FERROSEEK_DATA_DIR` override for
/// tests, benchmarks and isolated instances.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var).map(PathBuf::from) {
        // The spec says relative values are invalid and must be ignored.
        Some(p) if p.is_absolute() => p,
        _ => home().join(fallback),
    }
}

impl Paths {
    pub fn from_env() -> Paths {
        let config_file = std::env::var_os("FERROSEEK_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| xdg("XDG_CONFIG_HOME", ".config").join("ferroseek/config.toml"));
        let data_dir = std::env::var_os("FERROSEEK_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share").join("ferroseek"));
        Paths { config_file, data_dir }
    }

    pub fn index_file(&self) -> PathBuf {
        self.data_dir.join("index.sqlite")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join("index.lock")
    }

    /// Creates the data directory with mode 0700 (and tightens it if it
    /// already exists with looser permissions).
    pub fn ensure_data_dir(&self) -> io::Result<()> {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.data_dir)?;
        let m = fs::metadata(&self.data_dir)?;
        if m.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(&self.data_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

/// User configuration (`config.toml`). Unknown keys are rejected so typos
/// do not silently do nothing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Directories to index. `~` expands to `$HOME`.
    pub roots: Vec<String>,
    /// Index dotfiles and dot-directories.
    pub index_hidden: bool,
    /// File or directory *names* to skip; `*` and `?` wildcards.
    pub exclude_names: Vec<String>,
    /// Absolute directories to skip (with everything below). `~` expands.
    pub exclude_paths: Vec<String>,
    /// Skip directories containing a valid `CACHEDIR.TAG`
    /// (<https://bford.info/cachedir/>), e.g. Cargo `target/`.
    pub respect_cachedir_tag: bool,
    /// Do not descend into mount points below a root (other filesystems,
    /// network/FUSE/removable media, bind mounts). Btrfs subvolumes that are
    /// not separately mounted are not mount points and are indexed.
    pub stay_on_filesystem: bool,
    /// Maximum directory depth below a root.
    pub max_depth: u32,
    /// Duty-cycle limit for scanning, 1–100 (% of one core).
    pub max_cpu_percent: u32,
    /// Run the indexer thread at nice 19 and idle I/O priority.
    pub low_priority: bool,
    /// Coalescing window for filesystem events, in milliseconds.
    pub debounce_ms: u64,
    /// Periodic safety reconciliation, in minutes (0 disables).
    pub reconcile_interval_minutes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            roots: vec!["~".into()],
            index_hidden: false,
            exclude_names: DEFAULT_EXCLUDE_NAMES.iter().map(|s| s.to_string()).collect(),
            exclude_paths: vec![],
            respect_cachedir_tag: true,
            stay_on_filesystem: true,
            max_depth: 64,
            max_cpu_percent: 50,
            low_priority: true,
            debounce_ms: 200,
            reconcile_interval_minutes: 360,
        }
    }
}

/// Default name exclusions: VCS metadata, dependency/build trees, editor
/// and download temporaries, compiled objects. Written for FerroSeek (not
/// copied from Baloo's list), covering the same categories.
pub const DEFAULT_EXCLUDE_NAMES: &[&str] = &[
    // version control
    ".git",
    ".hg",
    ".svn",
    ".bzr",
    "_darcs",
    "CVS",
    // dependency and build trees
    "node_modules",
    "bower_components",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".gradle",
    "CMakeFiles",
    ".ccache",
    ".cargo",
    // temporaries
    "*~",
    "*.swp",
    "*.swo",
    "*.tmp",
    "*.part",
    "*.crdownload",
    ".#*",
    "#*#",
    // compiled objects
    "*.o",
    "*.obj",
    "*.pyc",
    "*.pyo",
    "*.class",
    "*.so",
    "*.a",
    "*.la",
    "*.lo",
    // misc
    "lost+found",
    ".Trash-*",
];

#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, io::Error),
    Parse(PathBuf, String),
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(p, e) => write!(f, "cannot read {}: {e}", p.display()),
            ConfigError::Parse(p, e) => write!(f, "invalid config {}: {e}", p.display()),
            ConfigError::Invalid(e) => write!(f, "invalid config: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Loads `path`; a missing file means defaults.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match fs::read_to_string(path) {
            Ok(s) => Config::parse(&s).map_err(|e| match e {
                ConfigError::Parse(_, m) => ConfigError::Parse(path.to_path_buf(), m),
                other => other,
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(path.to_path_buf(), e)),
        }
    }

    pub fn parse(s: &str) -> Result<Config, ConfigError> {
        let c: Config = toml::from_str(s).map_err(|e| ConfigError::Parse(PathBuf::new(), e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=100).contains(&self.max_cpu_percent) {
            return Err(ConfigError::Invalid("max_cpu_percent must be 1..=100".into()));
        }
        if self.max_depth == 0 {
            return Err(ConfigError::Invalid("max_depth must be > 0".into()));
        }
        for r in self.roots.iter().chain(&self.exclude_paths) {
            if !expand(r).is_absolute() {
                return Err(ConfigError::Invalid(format!("{r:?} is not an absolute path")));
            }
        }
        Ok(())
    }

    /// Expanded, de-duplicated roots, with roots nested inside other roots
    /// dropped (they would be indexed twice).
    pub fn root_paths(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.roots.iter().map(|r| normalize(&expand(r))).collect();
        roots.sort();
        roots.dedup();
        let all = roots.clone();
        roots.retain(|r| !all.iter().any(|o| o != r && r.starts_with(o)));
        roots
    }
}

/// `~` and `~/x` expansion.
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Lexical normalization: removes `.` components, trailing slashes and
/// resolves `..` without touching the filesystem (symlinks stay as named).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

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
    fn config_parse_validate_roundtrip() {
        let c = Config::parse("roots = [\"/data\", \"/data/sub\", \"/other/\"]\nmax_cpu_percent = 20\n").unwrap();
        assert_eq!(c.root_paths(), vec![PathBuf::from("/data"), PathBuf::from("/other")]);
        assert!(Config::parse("max_cpu_percent = 0").is_err());
        assert!(Config::parse("roots = [\"relative\"]").is_err());
        assert!(Config::parse("unknown_key = 1").is_err());
        let d = Config::default();
        assert_eq!(Config::parse(&d.to_toml()).unwrap(), d);
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
