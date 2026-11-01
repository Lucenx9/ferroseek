// SPDX-License-Identifier: GPL-3.0-or-later
//! Installed applications from freedesktop `.desktop` files.
//!
//! Implements the parts of the Desktop Entry Specification 1.5 a launcher
//! needs: XDG data-dir precedence by desktop-file ID, localized keys,
//! `Hidden`/`NoDisplay`/`OnlyShowIn`/`NotShowIn`/`TryExec`, value escapes
//! and the `Exec` quoting and field-code rules. Written here instead of
//! using a crate so the launcher builds no C code.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::matcher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// Desktop-file ID, e.g. `org.kde.dolphin.desktop`.
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub keywords: Vec<String>,
    pub icon: Option<String>,
    pub exec: String,
    pub actions: Vec<DesktopAction>,
    pub terminal: bool,
    pub working_dir: Option<PathBuf>,
    pub file: PathBuf,
    /// Lower-cased labels for matching, computed once.
    search: Vec<(String, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopAction {
    pub id: String,
    pub name: String,
    pub exec: String,
}

impl App {
    /// Matching score for query tokens, or `None`.
    pub fn score(&self, tokens: &[String]) -> Option<u32> {
        // The whole name typed out is an exact match even when it has
        // several words (token scores alone would average below exact).
        if !tokens.is_empty() && tokens.iter().map(String::as_str).eq(self.search[0].0.split_whitespace()) {
            return Some(matcher::EXACT);
        }
        let labels: Vec<(&str, u32)> = self.search.iter().map(|(l, w)| (l.as_str(), *w)).collect();
        matcher::score(tokens, &labels)
    }

    /// Argument vector for launching without files, per the `Exec` rules.
    pub fn argv(&self) -> Result<Vec<String>, ExecError> {
        expand_exec(&self.exec, self.icon.as_deref(), &self.name, &self.file)
    }

    pub fn action_argv(&self, index: usize) -> Result<Vec<String>, ExecError> {
        let action = self.actions.get(index).ok_or(ExecError::MissingAction)?;
        let mut chars = action.exec.chars();
        while let Some(c) = chars.next() {
            if c == '%'
                && !matches!(
                    chars.next(),
                    Some('%' | 'f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm' | 'i' | 'c' | 'k')
                )
            {
                return Err(ExecError::InvalidFieldCode);
            }
        }
        let argv = expand_exec(&action.exec, self.icon.as_deref(), &self.name, &self.file)?;
        if argv.iter().any(|arg| arg.contains('\0')) {
            return Err(ExecError::Nul);
        }
        Ok(argv)
    }
}

/// `$XDG_DATA_HOME` then `$XDG_DATA_DIRS`, with the specification defaults.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => dirs.push(PathBuf::from(d)),
        None => {
            if let Some(h) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(h).join(".local/share"));
            }
        }
    }
    let sys = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
    dirs.extend(std::env::split_paths(&sys).filter(|p| p.is_absolute()));
    dirs
}

/// Locale lookup order for `Key[locale]`: `lang_COUNTRY@MODIFIER`,
/// `lang_COUNTRY`, `lang@MODIFIER`, `lang`.
pub fn locale_variants(raw: &str) -> Vec<String> {
    // lang_COUNTRY.ENCODING@MODIFIER; the encoding is irrelevant.
    let (no_mod, modifier) = match raw.split_once('@') {
        Some((b, m)) => (b, Some(m)),
        None => (raw, None),
    };
    let base = no_mod.split('.').next().unwrap_or("");
    let (lang, country) = match base.split_once('_') {
        Some((l, c)) => (l, Some(c)),
        None => (base, None),
    };
    if lang.is_empty() || lang == "C" || lang == "POSIX" {
        return Vec::new();
    }
    let mut v = Vec::new();
    if let (Some(c), Some(m)) = (country, modifier) {
        v.push(format!("{lang}_{c}@{m}"));
    }
    if let Some(c) = country {
        v.push(format!("{lang}_{c}"));
    }
    if let Some(m) = modifier {
        v.push(format!("{lang}@{m}"));
    }
    v.push(lang.to_string());
    v
}

/// The process's message locale (`LC_ALL`, `LC_MESSAGES`, `LANG`).
pub fn current_locales() -> Vec<String> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return locale_variants(&v);
        }
    }
    Vec::new()
}

/// `XDG_CURRENT_DESKTOP` names, e.g. `["KDE"]`.
pub fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.split(':').filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Scans `<dir>/applications` of every data dir. The first file with a
/// given ID wins, even when it is hidden: that is how users mask system
/// entries.
pub fn scan(dirs: &[PathBuf], locales: &[String], desktops: &[String]) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let root = dir.join("applications");
        let mut files = Vec::new();
        collect_desktop_files(&root, &root, &mut files, 0);
        files.sort();
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if let Some(app) = parse(&id, &path, &text, locales, desktops) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
    apps
}

fn collect_desktop_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>, depth: u32) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        // Follow symlinks for files (common in /usr/share/applications)
        // but not for directories, so a cycle cannot recurse.
        if ft.is_dir() {
            collect_desktop_files(root, &path, out, depth + 1);
        } else if path.extension().is_some_and(|x| x == "desktop")
            && let Ok(rel) = path.strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push((rel.replace('/', "-"), path));
        }
    }
}

/// Parses one desktop file into a launchable, visible application.
pub fn parse(id: &str, file: &Path, text: &str, locales: &[String], desktops: &[String]) -> Option<App> {
    let group = entry_group(text, "Desktop Entry");
    let get = |key: &str| -> Option<String> { group.iter().find(|(k, _)| k == key).map(|(_, v)| unescape(v)) };
    let get_localized = |key: &str| -> Option<String> {
        for l in locales {
            let k = format!("{key}[{l}]");
            if let Some(v) = get(&k) {
                return Some(v);
            }
        }
        get(key)
    };
    let get_bool = |key: &str| get(key).is_some_and(|v| v == "true");

    if get("Type").as_deref() != Some("Application") || get_bool("Hidden") || get_bool("NoDisplay") {
        return None;
    }
    if let Some(only) = get("OnlyShowIn")
        && !split_list(&only).iter().any(|d| desktops.contains(d))
    {
        return None;
    }
    if let Some(not) = get("NotShowIn")
        && split_list(&not).iter().any(|d| desktops.contains(d))
    {
        return None;
    }
    if let Some(try_exec) = get("TryExec")
        && !executable_exists(&try_exec)
    {
        return None;
    }
    let name = get_localized("Name")?;
    let exec = get("Exec")?;
    let generic_name = get_localized("GenericName").filter(|s| !s.is_empty());
    let comment = get_localized("Comment").filter(|s| !s.is_empty());
    let keywords = get_localized("Keywords").map(|k| split_list(&k)).unwrap_or_default();
    let mut search = vec![(name.to_lowercase(), 100)];
    if let Some(g) = &generic_name {
        search.push((g.to_lowercase(), 70));
    }
    for k in &keywords {
        search.push((k.to_lowercase(), 60));
    }
    // The ID's last component ("org.kde.dolphin" → "dolphin") finds apps
    // by their executable-like name.
    let stem = id.trim_end_matches(".desktop");
    let short = stem.rsplit('.').next().unwrap_or(stem).to_lowercase();
    if short != name.to_lowercase() {
        search.push((short, 60));
    }
    if let Some(c) = &comment {
        search.push((c.to_lowercase(), 40));
    }
    let mut app = App {
        id: id.to_string(),
        name,
        generic_name,
        comment,
        keywords,
        icon: get("Icon").filter(|s| !s.is_empty()),
        exec,
        actions: Vec::new(),
        terminal: get_bool("Terminal"),
        working_dir: get("Path").filter(|p| !p.is_empty()).map(PathBuf::from),
        file: file.to_path_buf(),
        search,
    };
    let mut seen = HashSet::new();
    for id in get("Actions").map(|value| split_list(&value)).unwrap_or_default() {
        if !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') || !seen.insert(id.clone()) {
            continue;
        }
        let group = entry_group(text, &format!("Desktop Action {id}"));
        let get = |key: &str| group.iter().find(|(k, _)| k == key).map(|(_, v)| unescape(v));
        let name = locales.iter().find_map(|locale| get(&format!("Name[{locale}]"))).or_else(|| get("Name"));
        let (Some(name), Some(exec)) = (name, get("Exec")) else { continue };
        if name.trim().is_empty() || name.contains('\0') {
            continue;
        }
        app.actions.push(DesktopAction { id, name, exec });
        if app.action_argv(app.actions.len() - 1).is_err() {
            app.actions.pop();
        }
    }
    Some(app)
}

/// Raw `key=value` pairs of the first named group, unescaped by the caller.
fn entry_group(text: &str, name: &str) -> Vec<(String, String)> {
    let mut in_group = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if in_group {
                break;
            }
            in_group = line.trim_end().strip_prefix('[').and_then(|s| s.strip_suffix(']')) == Some(name);
            continue;
        }
        if in_group && let Some((k, v)) = line.split_once('=') {
            let k = k.trim_end().to_string();
            if !out.iter().any(|(ek, _): &(String, String)| *ek == k) {
                out.push((k, v.trim_start().to_string()));
            }
        }
    }
    out
}

/// String escapes: `\s \n \t \r \\`. `\;` is kept for list splitting.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push_str("\\;"),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `a;b\;c;` → `["a", "b;c"]`.
fn split_list(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut it = v.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && it.peek() == Some(&';') {
            cur.push(';');
            it.next();
        } else if c == ';' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|s| !s.is_empty());
    out
}

fn executable_exists(prog: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if prog.contains('/') {
        return is_exec(Path::new(prog));
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| is_exec(&d.join(prog))))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExecError {
    UnterminatedQuote,
    Empty,
    MissingAction,
    Nul,
    InvalidFieldCode,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::UnterminatedQuote => f.write_str("unterminated quote in Exec"),
            ExecError::Empty => f.write_str("empty Exec"),
            ExecError::MissingAction => f.write_str("application action no longer exists"),
            ExecError::Nul => f.write_str("NUL in Exec argument"),
            ExecError::InvalidFieldCode => f.write_str("invalid field code in Exec"),
        }
    }
}

/// Splits an (already string-unescaped) `Exec` value into arguments and
/// expands field codes for a launch without files or URLs.
pub fn expand_exec(exec: &str, icon: Option<&str>, name: &str, file: &Path) -> Result<Vec<String>, ExecError> {
    // 1. Tokenize: double quotes group, and inside them `\"`, `\``, `\$`
    //    and `\\` are escapes.
    let mut raw: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_arg = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' => quoted = false,
                '\\' if matches!(chars.peek(), Some('"' | '`' | '$' | '\\')) => cur.push(chars.next().unwrap_or('\\')),
                _ => cur.push(c),
            }
        } else if c == '"' {
            quoted = true;
            in_arg = true;
        } else if c == ' ' || c == '\t' || c == '\n' {
            if in_arg {
                raw.push(std::mem::take(&mut cur));
                in_arg = false;
            }
        } else {
            cur.push(c);
            in_arg = true;
        }
    }
    if quoted {
        return Err(ExecError::UnterminatedQuote);
    }
    if in_arg {
        raw.push(cur);
    }

    // 2. Field codes. A standalone code that expands to nothing removes the
    //    argument; `%i` expands to two arguments.
    let mut argv = Vec::new();
    for arg in raw {
        match arg.as_str() {
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => continue,
            "%i" => {
                if let Some(i) = icon {
                    argv.push("--icon".to_string());
                    argv.push(i.to_string());
                }
                continue;
            }
            _ => {}
        }
        let mut out = String::new();
        let mut it = arg.chars();
        while let Some(c) = it.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('%') => out.push('%'),
                Some('c') => out.push_str(name),
                Some('k') => out.push_str(&file.to_string_lossy()),
                Some('i') => {
                    if let Some(i) = icon {
                        out.push_str(i);
                    }
                }
                // Embedded file/URL and deprecated codes expand to nothing.
                Some(_) | None => {}
            }
        }
        argv.push(out);
    }
    if argv.is_empty() || argv[0].is_empty() {
        return Err(ExecError::Empty);
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Option<App> {
        parse(
            "org.example.App.desktop",
            Path::new("/x/org.example.App.desktop"),
            text,
            &locale_variants("it_IT.UTF-8"),
            &["KDE".to_string()],
        )
    }

    const BASIC: &str = "[Desktop Entry]\nType=Application\nName=Files\nName[it]=File\nGenericName=File Manager\nKeywords=folder;explorer;\nIcon=system-file-manager\nExec=files %U\n\n[Desktop Action new]\nName=Other\nExec=other\n";

    #[test]
    fn parses_localized_entry() {
        let a = p(BASIC).unwrap();
        assert_eq!(a.name, "File");
        assert_eq!(a.generic_name.as_deref(), Some("File Manager"));
        assert_eq!(a.keywords, ["folder", "explorer"]);
        assert_eq!(a.icon.as_deref(), Some("system-file-manager"));
        assert_eq!(a.argv().unwrap(), ["files"]);
        assert!(a.actions.is_empty(), "undeclared action groups are ignored");
        assert!(a.score(&matcher::tokens("explorer")).is_some());
        assert!(a.score(&matcher::tokens("app")).is_some(), "ID stem is searchable");
    }

    #[test]
    fn visibility_rules() {
        let with = |extra: &str| p(&format!("[Desktop Entry]\nType=Application\nName=A\nExec=a\n{extra}\n"));
        assert!(with("").is_some());
        assert!(with("NoDisplay=true").is_none());
        assert!(with("Hidden=true").is_none());
        assert!(with("OnlyShowIn=GNOME;").is_none());
        assert!(with("OnlyShowIn=GNOME;KDE;").is_some());
        assert!(with("NotShowIn=KDE;").is_none());
        assert!(with("TryExec=/nonexistent/ferroseek-test-binary").is_none());
        assert!(p("[Desktop Entry]\nType=Link\nName=A\nURL=x\n").is_none());
        assert!(p("[Desktop Entry]\nType=Application\nName=A\n").is_none(), "no Exec");
    }

    #[test]
    fn first_group_only_and_first_key_wins() {
        let a = p("# c\n[Desktop Entry]\nType=Application\nName=A\nName=B\nExec=a\n[Other]\nName=C\n").unwrap();
        assert_eq!(a.name, "A");
    }

    #[test]
    fn actions_follow_declaration_order_and_keep_parent_metadata() {
        let a = p("[Desktop Action new]\nName=New window\nExec=browser --new\n\
             [Desktop Entry]\nType=Application\nName=Browser\nName[it]=Navigatore\n\
             Exec=browser\nIcon=browser-icon\nPath=/parent\nActions=private;new;private;missing;\n\
             [Desktop Action private]\nName=Private window\nName[it]=Finestra privata\n\
             Name[it]=Duplicate\nExec=browser --private\nIcon=action-icon\nTerminal=true\nPath=/action\n\
             [Desktop Action extra]\nName=Extra\nExec=extra\n\
             [Desktop Entry]\nName=Replacement\nExec=replacement\nActions=extra;\n\
             [Desktop Action new]\nName=Replacement action\nExec=replacement\n")
        .unwrap();
        let actions: Vec<_> = a.actions.iter().map(|action| (action.id.as_str(), action.name.as_str())).collect();
        assert_eq!(actions, [("private", "Finestra privata"), ("new", "New window")]);
        assert_eq!(a.name, "Navigatore");
        assert_eq!(a.argv().unwrap(), ["browser"]);
        assert_eq!(a.icon.as_deref(), Some("browser-icon"));
        assert!(!a.terminal);
        assert_eq!(a.working_dir.as_deref(), Some(Path::new("/parent")));
        assert_eq!(a.action_argv(1).unwrap(), ["browser", "--new"]);
    }

    #[test]
    fn incomplete_and_unusable_actions_are_ignored() {
        let a = p("[Desktop Entry]\nType=Application\nName=A\nExec=a\n\
             Actions=noname;emptyname;noexec;emptyexec;onlyfiles;quote;nul;unknown;dangling;ok;\n\
             [Desktop Action noname]\nExec=a\n\
             [Desktop Action emptyname]\nName=\\s\nExec=a\n\
             [Desktop Action noexec]\nName=No command\n\
             [Desktop Action emptyexec]\nName=Empty command\nExec=   \n\
             [Desktop Action onlyfiles]\nName=Files\nExec=%U\n\
             [Desktop Action quote]\nName=Quote\nExec=a \"unclosed\n\
             [Desktop Action nul]\nName=Nul\nExec=a bad\0arg\n\
             [Desktop Action unknown]\nName=Unknown code\nExec=a %Z\n\
             [Desktop Action dangling]\nName=Incomplete code\nExec=a %\n\
             [Desktop Action ok]\nName=Valid\nExec=a --valid\n")
        .unwrap();
        assert_eq!(a.actions.len(), 1);
        assert_eq!(a.actions[0].id, "ok");
    }

    #[test]
    fn action_argv_uses_application_field_codes_without_a_shell() {
        let a = p("[Desktop Entry]\nType=Application\nName=Browser\nName[it]=Il browser\n\
             Exec=browser\nIcon=main-icon\nActions=new;\n\
             [Desktop Action new]\nName=New window\nIcon=action-icon\n\
             Exec=\"/opt/My App/bin\" --new %c %i %k %U \"x; $literal\" 100%%\n")
        .unwrap();
        assert_eq!(
            a.action_argv(0).unwrap(),
            [
                "/opt/My App/bin",
                "--new",
                "Il browser",
                "--icon",
                "main-icon",
                "/x/org.example.App.desktop",
                "x; $literal",
                "100%",
            ]
        );
        assert_eq!(a.action_argv(1), Err(ExecError::MissingAction));
        assert_eq!(a.action_argv(usize::MAX), Err(ExecError::MissingAction));
    }

    #[test]
    fn locale_variants_order() {
        assert_eq!(locale_variants("sr_RS.UTF-8@latin"), ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]);
        assert_eq!(locale_variants("it_IT.UTF-8"), ["it_IT", "it"]);
        assert!(locale_variants("C.UTF-8").is_empty());
    }

    #[test]
    fn exec_quoting_and_field_codes() {
        let f = Path::new("/apps/x.desktop");
        let e = |s: &str| expand_exec(&unescape(s), Some("icon-x"), "Name X", f);
        assert_eq!(e("prog %f").unwrap(), ["prog"]);
        assert_eq!(e("prog --open=%u --name %c").unwrap(), ["prog", "--open=", "--name", "Name X"]);
        assert_eq!(e("prog %i %k").unwrap(), ["prog", "--icon", "icon-x", "/apps/x.desktop"]);
        assert_eq!(e("prog 100%%").unwrap(), ["prog", "100%"]);
        assert_eq!(e(r#""/opt/My App/bin" "a \"q\" \\$x""#).unwrap(), ["/opt/My App/bin", "a \"q\" $x"]);
        // A desktop-file `\\` is one backslash at the Exec level.
        assert_eq!(e(r#"sh -c "echo \\\\""#).unwrap(), ["sh", "-c", "echo \\"]);
        assert_eq!(e(r#"prog "open"#), Err(ExecError::UnterminatedQuote));
        assert_eq!(e("%U"), Err(ExecError::Empty));
    }

    #[test]
    fn lists_and_escapes() {
        assert_eq!(split_list(&unescape(r"a;b\;c;;d")), ["a", "b;c", "d"]);
        assert_eq!(unescape(r"a\sb\tc\\d"), "a b\tc\\d");
    }

    #[test]
    fn scan_masks_by_id_and_recurses() {
        let home = tempfile::tempdir().unwrap();
        let sys = tempfile::tempdir().unwrap();
        let w = |root: &Path, rel: &str, body: &str| {
            let p = root.join("applications").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        let app = |n: &str| format!("[Desktop Entry]\nType=Application\nName={n}\nExec={n}\n");
        w(sys.path(), "a.desktop", &app("SysA"));
        w(sys.path(), "b.desktop", &app("SysB"));
        w(sys.path(), "vendor/c.desktop", &app("VendorC"));
        w(home.path(), "a.desktop", &app("UserA"));
        w(home.path(), "b.desktop", "[Desktop Entry]\nType=Application\nName=B\nExec=b\nHidden=true\n");
        let apps = scan(&[home.path().to_path_buf(), sys.path().to_path_buf()], &[], &[]);
        let names: Vec<_> = apps.iter().map(|a| (a.id.as_str(), a.name.as_str())).collect();
        assert_eq!(names, [("a.desktop", "UserA"), ("vendor-c.desktop", "VendorC")]);
    }
}
