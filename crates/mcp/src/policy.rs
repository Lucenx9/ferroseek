// SPDX-License-Identifier: GPL-3.0-or-later
//! Access policy for MCP clients.
//!
//! Being in the index does not authorize an agent to see a path. A path is
//! visible only when it is strictly below an explicitly allowed directory,
//! and no component *below* that directory is a dotfile (unless
//! `allow_hidden`) or matches a deny rule (secrets, keys, credentials).
//! Everything is lexical: the index never follows symlinks, so containment
//! is decided on the same path form the index stores.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use ferroseek_engine::config::{expand, glob_match};
use serde::Deserialize;

/// Name patterns (per path component, `*`/`?` wildcards, ASCII
/// case-insensitive) hidden from agents by default, even inside allowed
/// directories.
pub const DEFAULT_DENY: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".password-store",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".gcloud",
    ".netrc",
    ".pgpass",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".vault-token",
    ".env",
    ".env.*",
    "*.env",
    "id_rsa*",
    "id_dsa*",
    "id_ecdsa*",
    "id_ed25519*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.kdbx",
    "*.keystore",
    "*.jks",
    "*secret*",
    "*credential*",
];

/// Hard upper bound on results per call, whatever the configuration says.
pub const HARD_MAX_RESULTS: usize = 200;
/// Maximum number of allowed directories (each costs one index query).
pub const MAX_ALLOWED_DIRS: usize = 32;
/// Maximum query length in bytes.
pub const MAX_QUERY_BYTES: usize = 512;
/// Maximum accepted input path length in bytes (PATH_MAX).
pub const MAX_PATH_BYTES: usize = 4096;

/// `~/.config/ferroseek/mcp.toml`. Unknown keys are rejected.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    /// Directories agents may search. `~` expands. Nothing by default.
    pub allow: Vec<String>,
    /// Extra deny patterns (added to the defaults).
    pub deny: Vec<String>,
    /// Set to `false` to drop the built-in deny list.
    pub deny_defaults: Option<bool>,
    /// Show dotfiles and dot-directories below allowed directories.
    pub allow_hidden: bool,
    /// Default/maximum results per call (capped at `HARD_MAX_RESULTS`).
    pub max_results: Option<usize>,
    /// Report whole-index totals, size and last write time in
    /// `index_status`. Off by default: they describe unauthorized
    /// directories too.
    pub expose_index_totals: bool,
}

#[derive(Clone, Debug)]
pub struct Policy {
    allow: Vec<PathBuf>,
    deny: Vec<Pattern>,
    allow_hidden: bool,
    pub max_results: usize,
    pub expose_index_totals: bool,
}

/// Why a path or argument was refused. Messages never echo paths that
/// are outside the allowed directories.
#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    NotAbsolute,
    Malformed,
    NotAllowed,
    NoAllowedDirs,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Denied::NotAbsolute => "path must be absolute",
            Denied::Malformed => "path must not contain '.', '..', empty components or NUL bytes",
            Denied::NotAllowed => "path is outside the directories authorized for MCP access (or is filtered)",
            Denied::NoAllowedDirs => {
                "no directories are authorized for MCP access; start ferroseek-mcp with --allow DIR \
                 or set `allow` in ~/.config/ferroseek/mcp.toml"
            }
        })
    }
}

/// Validates a client-supplied absolute path: no `.`/`..`, no NUL, no
/// relative form. Trailing slashes are accepted and dropped.
pub fn parse_client_path(s: &str) -> Result<PathBuf, Denied> {
    if s.is_empty() || s.len() > MAX_PATH_BYTES || s.as_bytes().contains(&0) {
        return Err(Denied::Malformed);
    }
    let p = Path::new(s);
    if !p.is_absolute() {
        return Err(Denied::NotAbsolute);
    }
    // `Path::components` silently drops "." and repeated slashes; check the
    // raw segments so "/a/./b" and "/a//b" are rejected as non-canonical
    // rather than normalized.
    let trimmed = s.trim_end_matches('/');
    for seg in trimmed.split('/').skip(1) {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(Denied::Malformed);
        }
    }
    Ok(PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed }))
}

impl Policy {
    /// Builds the policy from the config file plus `--allow` arguments.
    /// Allowed entries must be absolute after `~` expansion and free of
    /// `.`/`..`; nested entries are collapsed into their ancestor.
    pub fn new(cfg: &FileConfig, extra_allow: &[String]) -> Result<Policy, String> {
        let mut allow = Vec::new();
        for raw in cfg.allow.iter().chain(extra_allow) {
            let expanded = expand(raw);
            let s = expanded.to_str().ok_or_else(|| format!("allowed directory {raw:?} is not valid UTF-8"))?;
            let p = parse_client_path(s).map_err(|e| format!("allowed directory {raw:?}: {e}"))?;
            if p == Path::new("/") {
                return Err("refusing to allow the whole filesystem (\"/\")".into());
            }
            allow.push(p);
        }
        allow.sort();
        allow.dedup();
        let all = allow.clone();
        allow.retain(|a| !all.iter().any(|o| o != a && a.starts_with(o)));
        if allow.len() > MAX_ALLOWED_DIRS {
            return Err(format!("at most {MAX_ALLOWED_DIRS} allowed directories are supported"));
        }
        let mut deny: Vec<Vec<u8>> = Vec::new();
        if cfg.deny_defaults.unwrap_or(true) {
            // Deny matching is ASCII case-insensitive (patterns and names are
            // compared lowercased): `CREDENTIALS/`, `server.PEM` must not slip by.
            deny.extend(DEFAULT_DENY.iter().map(|d| d.as_bytes().to_ascii_lowercase()));
        }
        deny.extend(cfg.deny.iter().map(|d| d.as_bytes().to_ascii_lowercase()));
        let deny = deny.into_iter().map(Pattern::compile).collect();
        let max_results = cfg.max_results.unwrap_or(50).clamp(1, HARD_MAX_RESULTS);
        Ok(Policy {
            allow,
            deny,
            allow_hidden: cfg.allow_hidden,
            max_results,
            expose_index_totals: cfg.expose_index_totals,
        })
    }

    pub fn allowed_dirs(&self) -> &[PathBuf] {
        &self.allow
    }

    /// The allowed directory containing `p`, if `p` is visible.
    ///
    /// `p` must be strictly below the directory (the directory itself is
    /// reported only through `index_status`), and every component below it
    /// must pass the hidden and deny rules.
    pub fn visible_under(&self, p: &Path) -> Option<&Path> {
        let base = self.allow.iter().find(|a| p.starts_with(a) && p != a.as_path())?;
        let rel = p.strip_prefix(base).ok()?;
        for c in rel.components() {
            let Component::Normal(name) = c else { return None };
            if !self.component_ok(name) {
                return None;
            }
        }
        Some(base)
    }

    /// Whether one path component (below an allowed directory) may be shown.
    pub fn name_ok(&self, name: &OsStr) -> bool {
        let b = name.as_bytes();
        if !self.allow_hidden && b.first() == Some(&b'.') {
            return false;
        }
        let lower = b.to_ascii_lowercase();
        !self.deny.iter().any(|d| d.matches(&lower))
    }

    fn component_ok(&self, name: &OsStr) -> bool {
        self.name_ok(name)
    }

    /// Whether children of directory `dir` can be visible, i.e. `dir` is an
    /// allowed directory or itself visible. Lets callers check a path as
    /// `dir_admits(parent) && name_ok(name)`, memoizing per directory.
    pub fn dir_admits(&self, dir: &Path) -> bool {
        self.allow.iter().any(|a| a == dir) || self.visible_under(dir).is_some()
    }

    /// Search scopes for an optional client `root` filter: the root itself
    /// if it is (or is inside) an allowed directory, otherwise every
    /// allowed directory.
    pub fn scopes(&self, root: Option<&str>) -> Result<Vec<PathBuf>, Denied> {
        if self.allow.is_empty() {
            return Err(Denied::NoAllowedDirs);
        }
        match root {
            None => Ok(self.allow.clone()),
            Some(r) => {
                let p = parse_client_path(r)?;
                if self.allow.contains(&p) || self.visible_under(&p).is_some() {
                    Ok(vec![p])
                } else {
                    Err(Denied::NotAllowed)
                }
            }
        }
    }
}

/// A lowercased deny pattern, pre-classified so the common shapes (exact,
/// `x*`, `*x`, `*x*`) avoid the general wildcard matcher. Semantics are
/// identical to `glob_match` on the lowercased name.
#[derive(Clone, Debug)]
enum Pattern {
    Exact(Vec<u8>),
    Prefix(Vec<u8>),
    Suffix(Vec<u8>),
    Contains(Vec<u8>),
    Glob(Vec<u8>),
}

impl Pattern {
    fn compile(p: Vec<u8>) -> Pattern {
        let wild = |b: &u8| *b == b'*' || *b == b'?';
        let inner = |s: &[u8]| !s.is_empty() && !s.iter().any(wild);
        match (p.first(), p.last()) {
            _ if !p.iter().any(wild) => Pattern::Exact(p),
            (Some(b'*'), Some(b'*')) if p.len() >= 2 && inner(&p[1..p.len() - 1]) => {
                Pattern::Contains(p[1..p.len() - 1].to_vec())
            }
            (_, Some(b'*')) if inner(&p[..p.len() - 1]) => Pattern::Prefix(p[..p.len() - 1].to_vec()),
            (Some(b'*'), _) if inner(&p[1..]) => Pattern::Suffix(p[1..].to_vec()),
            _ => Pattern::Glob(p),
        }
    }

    fn matches(&self, name: &[u8]) -> bool {
        match self {
            Pattern::Exact(x) => name == x.as_slice(),
            Pattern::Prefix(x) => name.starts_with(x),
            Pattern::Suffix(x) => name.ends_with(x),
            Pattern::Contains(x) => name.windows(x.len()).any(|w| w == x.as_slice()),
            Pattern::Glob(g) => glob_match(g, name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allow: &[&str]) -> Policy {
        Policy::new(&FileConfig { allow: allow.iter().map(|s| s.to_string()).collect(), ..Default::default() }, &[])
            .unwrap()
    }

    #[test]
    fn client_paths_are_strict() {
        assert_eq!(parse_client_path("/a/b/").unwrap(), Path::new("/a/b"));
        assert_eq!(parse_client_path("/").unwrap(), Path::new("/"));
        for bad in ["", "a/b", "./a", "/a/../b", "/a/./b", "/a//b", "/a/..", "/a\0b", "~/x"] {
            assert!(parse_client_path(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_client_path("rel"), Err(Denied::NotAbsolute));
        assert!(parse_client_path(&format!("/{}", "a".repeat(MAX_PATH_BYTES))).is_err());
    }

    #[test]
    fn containment_is_component_wise_and_strict() {
        let p = policy(&["/home/u/Progetti"]);
        assert!(p.visible_under(Path::new("/home/u/Progetti/ferro/src/main.rs")).is_some());
        assert!(p.visible_under(Path::new("/home/u/Progetti")).is_none(), "the dir itself is not a result");
        assert!(p.visible_under(Path::new("/home/u/ProgettiX/a")).is_none(), "sibling prefix");
        assert!(p.visible_under(Path::new("/home/u/Documenti/a")).is_none());
        assert!(p.visible_under(Path::new("/home/u")).is_none());
    }

    #[test]
    fn hidden_and_sensitive_components_are_filtered_below_the_allowed_dir() {
        let p = policy(&["/home/u/.config/app", "/srv/work"]);
        assert!(p.visible_under(Path::new("/home/u/.config/app/settings.json")).is_some(), "allowed dir may be hidden");
        assert!(p.visible_under(Path::new("/srv/work/.git/config")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/deploy/id_ed25519")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/certs/server.pem")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/my-secrets/x.txt")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/.env")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/prod.env")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/src/environment.rs")).is_some());
        // Case variants (review finding): deny matching ignores ASCII case.
        assert!(p.visible_under(Path::new("/srv/work/CREDENTIALS/record.txt")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/certs/server.PEM")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/private.KEY")).is_none());
        assert!(p.visible_under(Path::new("/srv/work/Top-Secret-Plan.md")).is_none());
    }

    #[test]
    fn config_options() {
        let cfg = FileConfig {
            allow: vec!["/srv/a".into(), "/srv/a/nested".into(), "/srv/b".into()],
            deny: vec!["*.sqlite".into()],
            deny_defaults: Some(false),
            allow_hidden: true,
            max_results: Some(10_000),
            expose_index_totals: false,
        };
        let p = Policy::new(&cfg, &["/srv/c".into()]).unwrap();
        assert_eq!(p.allowed_dirs(), &[PathBuf::from("/srv/a"), PathBuf::from("/srv/b"), PathBuf::from("/srv/c")]);
        assert_eq!(p.max_results, HARD_MAX_RESULTS);
        assert!(p.visible_under(Path::new("/srv/a/.git/x.pem")).is_some(), "defaults off, hidden on");
        assert!(p.visible_under(Path::new("/srv/a/db.sqlite")).is_none());
        assert!(p.visible_under(Path::new("/srv/a/DB.SQLITE")).is_none(), "user patterns are case-insensitive too");
        assert!(Policy::new(&FileConfig::default(), &["/".into()]).is_err());
        assert!(Policy::new(&FileConfig::default(), &["relative".into()]).is_err());
        assert!(Policy::new(&FileConfig::default(), &["/a/../etc".into()]).is_err());
        let parsed: FileConfig = toml::from_str("allow = [\"/x\"]\nallow_hidden = true\n").unwrap();
        assert_eq!(parsed.allow, vec!["/x".to_string()]);
        assert!(toml::from_str::<FileConfig>("alow = []").is_err(), "typos are rejected");
    }

    #[test]
    fn scopes() {
        let p = policy(&["/srv/a", "/srv/b"]);
        assert_eq!(p.scopes(None).unwrap().len(), 2);
        assert_eq!(p.scopes(Some("/srv/a/sub")).unwrap(), vec![PathBuf::from("/srv/a/sub")]);
        assert_eq!(p.scopes(Some("/srv/a")).unwrap(), vec![PathBuf::from("/srv/a")]);
        assert_eq!(p.scopes(Some("/srv")), Err(Denied::NotAllowed));
        assert_eq!(p.scopes(Some("/srv/a/../c")), Err(Denied::Malformed));
        assert_eq!(p.scopes(Some("/srv/a/.ssh")), Err(Denied::NotAllowed));
        assert_eq!(policy(&[]).scopes(None), Err(Denied::NoAllowedDirs));
    }

    proptest::proptest! {
        #[test]
        fn compiled_patterns_match_like_glob(
            pat in "[a-c.*?]{0,6}",
            name in proptest::collection::vec(proptest::sample::select(b"abc.xyz".to_vec()), 0..10),
        ) {
            let p = pat.as_bytes().to_vec();
            proptest::prop_assert_eq!(Pattern::compile(p.clone()).matches(&name), glob_match(&p, &name));
        }
    }

    #[test]
    fn dir_admits_and_name_ok_compose_to_visible_under() {
        let p = policy(&["/srv/a"]);
        for path in ["/srv/a/x", "/srv/a/.h/x", "/srv/a/k.pem", "/srv/a/d/e/f.txt", "/srv/a/secrets/x", "/srv/b/x"] {
            let path = Path::new(path);
            let composed = p.dir_admits(path.parent().unwrap()) && p.name_ok(path.file_name().unwrap());
            assert_eq!(composed, p.visible_under(path).is_some(), "{}", path.display());
        }
    }

    #[test]
    fn non_utf8_components_are_handled() {
        let p = policy(&["/srv/a"]);
        let raw = PathBuf::from(OsStr::from_bytes(b"/srv/a/caf\xe9.txt"));
        assert!(p.visible_under(&raw).is_some());
    }
}
