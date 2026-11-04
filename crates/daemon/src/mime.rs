// SPDX-License-Identifier: GPL-3.0-or-later
//! File name → MIME type → icon name, from the system's shared-mime-info
//! `globs2` database (no MIME crate, always in sync with the desktop).
//!
//! Only simple globs are used: `*.ext` suffixes (case-insensitive unless
//! flagged `cs`) and literal file names. That covers the overwhelming
//! majority of entries. Content sniffing is deliberately not done: it would
//! mean reading files during a keystroke.

use std::collections::{HashMap, HashSet};

use ferroseek_desktop::apps::data_dirs;

/// Candidates in directory-priority, weight and original line order.
type GlobMap = HashMap<String, Vec<(u32, String)>>;

pub struct MimeDb {
    /// Case-insensitive globs, keyed lowercase.
    by_ext: GlobMap,
    by_name: GlobMap,
    /// Case-sensitive (`cs`) globs, keyed verbatim.
    by_ext_cs: GlobMap,
    by_name_cs: GlobMap,
    generic: HashMap<String, String>,
    related: HashMap<String, Vec<String>>,
}

impl MimeDb {
    fn empty() -> MimeDb {
        MimeDb {
            by_ext: HashMap::new(),
            by_name: HashMap::new(),
            by_ext_cs: HashMap::new(),
            by_name_cs: HashMap::new(),
            generic: HashMap::new(),
            related: HashMap::new(),
        }
    }

    pub fn load() -> MimeDb {
        let mut db = MimeDb::empty();
        // Lower-priority dirs first so user dirs override.
        for dir in data_dirs().iter().rev() {
            if let Ok(s) = std::fs::read_to_string(dir.join("mime/globs2")) {
                db.add_globs2(&s);
            }
            if let Ok(s) = std::fs::read_to_string(dir.join("mime/generic-icons")) {
                for l in s.lines() {
                    if let Some((m, i)) = l.split_once(':') {
                        db.generic.insert(m.to_string(), i.to_string());
                    }
                }
            }
        }
        db
    }

    /// Explicit chooser reads include current alias/subclass metadata. Ordinary
    /// search icon lookups retain their existing cached, filename-only path.
    pub fn load_for_handlers() -> MimeDb {
        let mut db = Self::load();
        let mut aliases = HashMap::new();
        for dir in data_dirs().iter().rev() {
            if let Ok(text) = std::fs::read_to_string(dir.join("mime/aliases")) {
                for line in text.lines().filter(|line| !line.starts_with('#')) {
                    let mut fields = line.split_whitespace();
                    if let (Some(alias), Some(canonical)) = (fields.next(), fields.next()) {
                        aliases.insert(alias.to_string(), canonical.to_string());
                    }
                }
            }
            if let Ok(text) = std::fs::read_to_string(dir.join("mime/subclasses")) {
                for line in text.lines().filter(|line| !line.starts_with('#')) {
                    let mut fields = line.split_whitespace();
                    if let (Some(child), Some(parent)) = (fields.next(), fields.next()) {
                        db.related.entry(child.to_string()).or_default().push(parent.to_string());
                    }
                }
            }
        }
        // Aliases are equivalent in either direction; a later user alias
        // replaces its system definition before reverse edges are constructed.
        for (alias, canonical) in aliases {
            db.related.entry(alias.clone()).or_default().push(canonical.clone());
            db.related.entry(canonical).or_default().push(alias);
        }
        db
    }

    pub fn handler_types(&self, name: &str) -> Vec<String> {
        let Some(mime) = self.mime_for(name) else { return Vec::new() };
        let mut pending = vec![mime.to_string()];
        let mut seen = HashSet::new();
        let mut types = Vec::new();
        while let Some(mime) = pending.pop() {
            if !seen.insert(mime.clone()) {
                continue;
            }
            if mime.starts_with("text/") && mime != "text/plain" {
                pending.push("text/plain".into());
            }
            if !mime.starts_with("inode/") && mime != "application/octet-stream" {
                pending.push("application/octet-stream".into());
            }
            if let Some(parents) = self.related.get(&mime) {
                pending.extend(parents.iter().cloned());
            }
            types.push(mime);
        }
        types
    }

    pub fn add_globs2(&mut self, s: &str) {
        let mut layer = Self::empty();
        let mut removed = HashSet::new();
        for line in s.lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut it = line.split(':');
            let (Some(w), Some(mime), Some(glob)) = (it.next(), it.next(), it.next()) else { continue };
            if glob == "__NOGLOBS__" {
                removed.insert(mime);
                continue;
            }
            let Ok(weight) = w.parse::<u32>() else { continue };
            let cs = it.next().is_some_and(|f| f.split(',').any(|x| x == "cs"));
            let (map, key) = if let Some(ext) = glob.strip_prefix("*.") {
                if ext.contains(['*', '?', '[']) {
                    continue;
                }
                if cs { (&mut layer.by_ext_cs, ext.to_string()) } else { (&mut layer.by_ext, ext.to_lowercase()) }
            } else if !glob.contains(['*', '?', '[']) {
                if cs { (&mut layer.by_name_cs, glob.to_string()) } else { (&mut layer.by_name, glob.to_lowercase()) }
            } else {
                continue;
            };
            map.entry(key).or_default().push((weight, mime.to_string()));
        }
        // Keep alternatives until resets have been applied, including rules
        // shadowed by a higher-priority directory or a higher-weight MIME type.
        for (previous, current) in [
            (&mut self.by_ext, layer.by_ext),
            (&mut self.by_name, layer.by_name),
            (&mut self.by_ext_cs, layer.by_ext_cs),
            (&mut self.by_name_cs, layer.by_name_cs),
        ] {
            previous.retain(|_, rules| {
                rules.retain(|(_, mime)| !removed.contains(mime.as_str()));
                !rules.is_empty()
            });
            for (key, mut rules) in current {
                // Stable sorting preserves the earlier line on equal weights.
                rules.sort_by_key(|(weight, _)| std::cmp::Reverse(*weight));
                let older = previous.entry(key).or_default();
                rules.append(older);
                *older = rules;
            }
        }
    }

    /// MIME type by literal name, then suffix weight and length.
    pub fn mime_for(&self, name: &str) -> Option<&str> {
        // Prefer case-sensitive literal matches only when their weights tie.
        if let Some((_, m)) = [self.by_name.get(&name.to_lowercase()), self.by_name_cs.get(name)]
            .into_iter()
            .flatten()
            .filter_map(|rules| rules.first())
            .max_by_key(|(weight, _)| *weight)
        {
            return Some(m);
        }
        // Longest suffix and case-sensitive matches win equal-weight ties.
        let mut best: Option<&(u32, String)> = None;
        for (i, _) in name.match_indices('.') {
            let ext = &name[i + 1..];
            if ext.is_empty() {
                continue;
            }
            for hit in [self.by_ext_cs.get(ext), self.by_ext.get(&ext.to_lowercase())]
                .into_iter()
                .flatten()
                .filter_map(|rules| rules.first())
            {
                if best.is_none_or(|(weight, _)| hit.0 > *weight) {
                    best = Some(hit);
                }
            }
        }
        best.map(|(_, mime)| mime.as_str())
    }

    /// Freedesktop icon name for a file name (`application/pdf` →
    /// `application-pdf`), or a generic icon.
    pub fn icon_for(&self, name: &str) -> String {
        match self.mime_for(name) {
            Some(m) => m.replace('/', "-"),
            None => "unknown".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn db() -> MimeDb {
        let mut d = MimeDb::empty();
        d.add_globs2(
            "# comment\n50:text/markdown:*.md\n10:application/x-genesis-rom:*.md\n50:application/pdf:*.pdf\n\
             50:application/x-compressed-tar:*.tar.gz\n50:application/gzip:*.gz\n50:text/x-makefile:makefile\n\
             50:text/x-genie:*.gs:cs\n50:image/png:*.png\n",
        );
        d.generic.insert("application/x-compressed-tar".into(), "package-x-generic".into());
        d
    }

    #[test]
    fn extra_globs2_fields_preserve_case_sensitive_flags() {
        let mut d = MimeDb::empty();
        d.add_globs2("50:text/x-upper:*.UPPER:cs,future:extension\n50:text/x-literal:README:cs:extension\n");
        assert_eq!(d.mime_for("sample.UPPER"), Some("text/x-upper"));
        assert_eq!(d.mime_for("sample.upper"), None);
        assert_eq!(d.mime_for("README"), Some("text/x-literal"));
        assert_eq!(d.mime_for("readme"), None);
        assert_eq!(d.icon_for("sample.upper"), "unknown");
        assert!(d.handler_types("sample.upper").is_empty());
    }

    #[test]
    fn lookup_rules() {
        let d = db();
        assert_eq!(d.mime_for("notes.md"), Some("text/markdown"), "highest weight wins");
        assert_eq!(d.mime_for("REPORT.PDF"), Some("application/pdf"), "case-insensitive");
        assert_eq!(d.mime_for("x.tar.gz"), Some("application/x-compressed-tar"), "longest ext first");
        assert_eq!(d.mime_for("x.gz"), Some("application/gzip"));
        assert_eq!(d.mime_for("Makefile"), Some("text/x-makefile"));
        assert_eq!(d.mime_for("a.GS"), None, "case-sensitive glob");
        assert_eq!(d.mime_for("a.gs"), Some("text/x-genie"));
        assert_eq!(d.mime_for("noext"), None);
        assert_eq!(d.icon_for("a.pdf"), "application-pdf");
        assert_eq!(d.icon_for("weird"), "unknown");
    }

    #[test]
    fn mime_loading_ignores_invalid_xdg_paths() {
        use std::process::Command;

        const CASE: &str = "FERROSEEK_TEST_MIME_DATA_HOME";
        if std::env::var_os(CASE).is_some() {
            assert_eq!(MimeDb::load().mime_for("file.probe"), Some("text/x-user"));
            assert!(MimeDb::load_for_handlers().handler_types("file.probe").contains(&"text/x-user".into()));
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        for (relative, glob) in [
            ("home/.local/share", "50:text/x-user:*.probe\n"),
            ("relative", "80:text/x-working-directory:*.probe\n"),
            ("", "80:text/x-working-directory:*.probe\n"),
            ("relative-system", "90:text/x-relative-system:*.probe\n"),
        ] {
            let mime = temp.path().join(relative).join("mime");
            std::fs::create_dir_all(&mime).unwrap();
            std::fs::write(mime.join("globs2"), glob).unwrap();
        }
        let system = temp.path().join("system");
        let data_dirs = std::env::join_paths([Path::new("relative-system"), system.as_path()]).unwrap();
        for data_home in [Path::new(""), Path::new("relative"), &temp.path().join("home/.local/share")] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "mime::tests::mime_loading_ignores_invalid_xdg_paths"])
                .current_dir(temp.path())
                .env(CASE, "1")
                .env("HOME", temp.path().join("home"))
                .env("XDG_DATA_HOME", data_home)
                .env("XDG_DATA_DIRS", &data_dirs)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        }
    }

    #[test]
    fn literal_weight_precedes_case_sensitivity() {
        let mut d = MimeDb::empty();
        d.add_globs2("20:text/x-legacy:Makefile:cs\n80:text/x-current:makefile\n");
        assert_eq!(d.mime_for("Makefile"), Some("text/x-current"));
        assert_eq!(d.icon_for("Makefile"), "text-x-current");
        assert!(d.handler_types("Makefile").contains(&"text/x-current".into()));
        assert!(!d.handler_types("Makefile").contains(&"text/x-legacy".into()));
        assert_eq!(d.mime_for("MAKEFILE"), Some("text/x-current"));

        d.add_globs2("80:text/x-exact:Makefile:cs\n");
        assert_eq!(d.mime_for("Makefile"), Some("text/x-exact"));
        assert_eq!(d.mime_for("MAKEFILE"), Some("text/x-current"));

        d.add_globs2("10:text/x-literal:build.file\n100:text/x-suffix:*.file\n");
        assert_eq!(d.mime_for("build.file"), Some("text/x-literal"), "literal names precede suffix patterns");
    }

    #[test]
    fn user_mime_rules_override_identical_system_patterns() {
        use std::process::Command;

        const CASE: &str = "FERROSEEK_TEST_MIME_PRECEDENCE";
        if std::env::var_os(CASE).is_some() {
            let db = MimeDb::load();
            for name in ["file.probe", "file.CASE", "Literal", "Exact"] {
                assert_eq!(db.mime_for(name), Some("text/x-user"), "wrong rule for {name}");
                assert_eq!(db.icon_for(name), "text-x-user");
            }
            assert_eq!(db.mime_for("untouched.other"), Some("text/x-system"));
            assert!(MimeDb::load_for_handlers().handler_types("file.probe").contains(&"text/x-user".into()));
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        for (directory, weight, mime_type) in [("system", 80, "text/x-system"), ("user", 50, "text/x-user")] {
            let mime = temp.path().join(directory).join("mime");
            std::fs::create_dir_all(&mime).unwrap();
            let mut globs = ["*.probe", "*.CASE:cs", "Literal", "Exact:cs"]
                .map(|pattern| format!("{weight}:{mime_type}:{pattern}\n"))
                .concat();
            if directory == "system" {
                globs.push_str("50:text/x-system:*.other\n");
            } else {
                globs.push_str("10:text/x-lower-weight:*.probe\n");
            }
            std::fs::write(mime.join("globs2"), globs).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "mime::tests::user_mime_rules_override_identical_system_patterns"])
            .env(CASE, "1")
            .env("HOME", temp.path())
            .env("XDG_DATA_HOME", temp.path().join("user"))
            .env("XDG_DATA_DIRS", temp.path().join("system"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }

    #[test]
    fn glob_deleteall_is_not_a_literal_filename() {
        let mut db = MimeDb::empty();
        db.add_globs2("0:text/x-diff:__NOGLOBS__\n50:text/x-diff:*.diff\n");
        assert_eq!(db.mime_for("__NOGLOBS__"), None);
        assert_eq!(db.icon_for("__NOGLOBS__"), "unknown");
        assert!(db.handler_types("__NOGLOBS__").is_empty());
        assert_eq!(db.mime_for("file.diff"), Some("text/x-diff"));
    }

    #[test]
    fn glob_deleteall_preserves_alternative_associations() {
        for (pattern, name) in
            [("*.probe", "file.probe"), ("*.PROBE:cs", "file.PROBE"), ("Probe", "Probe"), ("Probe:cs", "Probe")]
        {
            let mut db = MimeDb::empty();
            db.add_globs2(&format!(
                "80:text/x-removed:{pattern}\n40:text/x-low:{pattern}\n50:text/x-retained:{pattern}\n50:text/x-tied:{pattern}\n"
            ));
            assert_eq!(db.mime_for(name), Some("text/x-removed"));
            db.add_globs2("0:text/x-removed:__NOGLOBS__\n");
            assert_eq!(db.mime_for(name), Some("text/x-retained"), "lost alternative for {pattern}");
            assert_eq!(db.icon_for(name), "text-x-retained");
            assert!(db.handler_types(name).contains(&"text/x-retained".into()));

            db.add_globs2(&format!("10:text/x-user:{pattern}\n"));
            assert_eq!(db.mime_for(name), Some("text/x-user"), "directory priority precedes weight");
            db.add_globs2("0:text/x-user:__NOGLOBS__\n");
            assert_eq!(db.mime_for(name), Some("text/x-retained"), "lost alternative from an earlier directory");
            db.add_globs2("0:text/x-retained:__NOGLOBS__\n");
            assert_eq!(db.mime_for(name), Some("text/x-tied"), "equal weights preserve line order");
            db.add_globs2("0:text/x-tied:__NOGLOBS__\n");
            assert_eq!(db.mime_for(name), Some("text/x-low"));
        }
    }

    #[test]
    fn user_glob_deleteall_removes_previous_patterns_without_becoming_a_filename() {
        use std::process::Command;

        const CASE: &str = "FERROSEEK_TEST_MIME_DELETEALL";
        if std::env::var_os(CASE).is_some() {
            let db = MimeDb::load();
            for name in ["file.patch", "file.OLD", "OldLiteral", "OldExact", "file.removed"] {
                assert_eq!(db.mime_for(name), None, "deleted rule still matches {name}");
                assert_eq!(db.icon_for(name), "unknown");
                assert!(MimeDb::load_for_handlers().handler_types(name).is_empty());
            }
            assert_eq!(db.mime_for("__NOGLOBS__"), None, "a reset directive is not a filename rule");
            assert_eq!(db.mime_for("file.diff"), Some("text/x-diff"));
            assert_eq!(db.mime_for("file.keep"), Some("text/x-keep"));
            assert_eq!(db.mime_for("file.probe"), Some("text/x-retained"));
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        for (directory, globs) in [
            (
                "system",
                concat!(
                    "80:text/x-diff:*.patch\n80:text/x-diff:*.diff\n80:text/x-diff:*.OLD:cs\n",
                    "80:text/x-diff:OldLiteral\n80:text/x-diff:OldExact:cs\n",
                    "50:text/x-removed:*.removed\n50:text/x-keep:*.keep\n",
                    "80:text/x-removed:*.probe\n50:text/x-retained:*.probe\n",
                ),
            ),
            ("user", "0:text/x-diff:__NOGLOBS__\n50:text/x-diff:*.diff\n0:text/x-removed:__NOGLOBS__\n"),
        ] {
            let mime = temp.path().join(directory).join("mime");
            std::fs::create_dir_all(&mime).unwrap();
            std::fs::write(mime.join("globs2"), globs).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "mime::tests::user_glob_deleteall_removes_previous_patterns_without_becoming_a_filename"])
            .env(CASE, "1")
            .env("HOME", temp.path())
            .env("XDG_DATA_HOME", temp.path().join("user"))
            .env("XDG_DATA_DIRS", temp.path().join("system"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn suffix_weight_precedes_length_and_case_sensitivity() {
        let mut d = MimeDb::empty();
        d.add_globs2("50:application/x-compressed-tar:*.tar.gz\n80:application/gzip:*.gz\n");
        assert_eq!(d.mime_for("archive.tar.gz"), Some("application/gzip"));
        assert_eq!(d.icon_for("archive.tar.gz"), "application-gzip");
        assert!(d.handler_types("archive.tar.gz").contains(&"application/gzip".to_string()));

        d.add_globs2("40:text/x-specific:*.SRC:cs\n70:text/plain:*.src\n");
        assert_eq!(d.mime_for("file.SRC"), Some("text/plain"));
        assert_eq!(d.mime_for("file.src"), Some("text/plain"));
        d.add_globs2("90:text/x-specific:*.SRC:cs\n");
        assert_eq!(d.mime_for("file.SRC"), Some("text/x-specific"));
        assert_eq!(d.mime_for("file.src"), Some("text/plain"));
    }

    #[test]
    fn system_db_loads_if_present() {
        let d = MimeDb::load();
        if Path::new("/usr/share/mime/globs2").exists() {
            assert_eq!(d.mime_for("x.pdf"), Some("application/pdf"));
        }
    }
}
