// SPDX-License-Identifier: GPL-3.0-or-later
//! File name → MIME type → icon name, from the system's shared-mime-info
//! `globs2` database (no MIME crate, always in sync with the desktop).
//!
//! Only simple globs are used: `*.ext` suffixes (case-insensitive unless
//! flagged `cs`) and literal file names. That covers the overwhelming
//! majority of entries. Content sniffing is deliberately not done: it would
//! mean reading files during a keystroke.

use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct MimeDb {
    /// Case-insensitive globs, keyed lowercase.
    by_ext: HashMap<String, (u32, String)>,
    by_name: HashMap<String, (u32, String)>,
    /// Case-sensitive (`cs`) globs, keyed verbatim.
    by_ext_cs: HashMap<String, (u32, String)>,
    by_name_cs: HashMap<String, (u32, String)>,
    generic: HashMap<String, String>,
    related: HashMap<String, Vec<String>>,
}

fn data_dirs() -> Vec<std::path::PathBuf> {
    let mut v = vec![];
    if let Some(h) = std::env::var_os("XDG_DATA_HOME") {
        v.push(h.into());
    } else if let Some(h) = std::env::var_os("HOME") {
        v.push(Path::new(&h).join(".local/share"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(Into::into));
    v
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
        for line in s.lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut it = line.splitn(4, ':');
            let (Some(w), Some(mime), Some(glob)) = (it.next(), it.next(), it.next()) else { continue };
            let Ok(weight) = w.parse::<u32>() else { continue };
            let cs = it.next().is_some_and(|f| f.split(',').any(|x| x == "cs"));
            let (map, key) = if let Some(ext) = glob.strip_prefix("*.") {
                if ext.contains(['*', '?', '[']) {
                    continue;
                }
                if cs { (&mut self.by_ext_cs, ext.to_string()) } else { (&mut self.by_ext, ext.to_lowercase()) }
            } else if !glob.contains(['*', '?', '[']) {
                if cs { (&mut self.by_name_cs, glob.to_string()) } else { (&mut self.by_name, glob.to_lowercase()) }
            } else {
                continue;
            };
            // Highest weight wins; on ties the earlier line wins (globs2 is
            // sorted by weight, then by preference).
            match map.get(&key) {
                Some((ow, _)) if *ow >= weight => {}
                _ => {
                    map.insert(key, (weight, mime.to_string()));
                }
            }
        }
    }

    /// MIME type for a file name, by literal name, then longest extension.
    pub fn mime_for(&self, name: &str) -> Option<&str> {
        if let Some((_, m)) = self.by_name_cs.get(name).or_else(|| self.by_name.get(&name.to_lowercase())) {
            return Some(m);
        }
        // "a.tar.gz": try "tar.gz" before "gz"; also case-sensitive first.
        for (i, _) in name.match_indices('.') {
            let ext = &name[i + 1..];
            if ext.is_empty() {
                continue;
            }
            if let Some((_, m)) = self.by_ext_cs.get(ext).or_else(|| self.by_ext.get(&ext.to_lowercase())) {
                return Some(m);
            }
        }
        None
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
    fn system_db_loads_if_present() {
        let d = MimeDb::load();
        if Path::new("/usr/share/mime/globs2").exists() {
            assert_eq!(d.mime_for("x.pdf"), Some("application/pdf"));
        }
    }
}
