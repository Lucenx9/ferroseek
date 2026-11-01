// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-independent launcher state: result list, selection window, actions
//! and the keyboard map. Everything here is unit-tested without a display.

use std::ops::Range;

use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};

use crate::apps::App;
use crate::backend::FileHit;
use crate::usage::Kind;
use crate::{calc, matcher, uri};

/// Visible rows addressable by Ctrl+1 through Ctrl+8.
pub const NUMBERED_SLOTS: usize = 8;

/// Applications shown when there are no file results.
const MAX_APPS_ALONE: usize = 9;
/// With file results: strong application matches go above files, weak
/// ones (only a description or a loose subsequence matched) below them.
const MAX_STRONG_APPS: usize = 4;
const MAX_WEAK_APPS: usize = 3;
/// Name prefix or word match, or a keyword prefix (see `matcher`).
const STRONG_APP_SCORE: u32 = 500;
/// Loose subsequence-only matches score below this; they are noise once
/// anything matches strongly.
const SUBSEQUENCE_CEILING: u32 = 300;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Index into the launcher's application list.
    App(usize),
    File(FileHit),
    /// A calculator result, as displayed.
    Calc(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Calculator,
    Applications,
    Files,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub target: Target,
    pub section: Section,
    pub title: String,
    pub subtitle: String,
    pub icon: String,
    pub tag: &'static str,
}

/// Score of an exact whole-name match.
const EXACT_SCORE: u32 = matcher::EXACT;

/// Matching applications for `query` as `(index, match score)`, best
/// first. Launch history (`boost`, capped) only reorders matches of the
/// same class: it never lifts a match above an exact name, or a weak match
/// (description, loose subsequence) into the strong class.
pub fn match_apps(apps: &[App], query: &str, boost: impl Fn(&App) -> u32) -> Vec<(usize, u32)> {
    let tokens = matcher::tokens(query);
    let mut scored: Vec<(usize, u32, (bool, bool, u32))> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let base = a.score(&tokens)?;
            let rank = if base >= SUBSEQUENCE_CEILING { base + boost(a) } else { base };
            Some((i, base, (base >= EXACT_SCORE, base >= STRONG_APP_SCORE, rank)))
        })
        .collect();
    // Stable: equal keys keep the alphabetical scan order.
    scored.sort_by_key(|&(_, _, key)| std::cmp::Reverse(key));
    scored.into_iter().map(|(i, base, _)| (i, base)).collect()
}

fn app_item(apps: &[App], i: usize) -> Item {
    let a = &apps[i];
    Item {
        target: Target::App(i),
        section: Section::Applications,
        title: a.name.clone(),
        subtitle: a.comment.clone().or_else(|| a.generic_name.clone()).unwrap_or_default(),
        icon: a.icon.clone().unwrap_or_default(),
        tag: "Application",
    }
}

fn file_item(f: &FileHit) -> Item {
    Item {
        target: Target::File(f.clone()),
        section: Section::Files,
        title: f.name.clone(),
        subtitle: f.subtext.clone(),
        icon: f.icon.clone(),
        tag: if f.is_dir { "Folder" } else { "File" },
    }
}

/// Moves files the user opens often up, keeping the daemon's order among
/// equals.
pub fn order_files(files: &mut [FileHit], boost: impl Fn(&FileHit) -> u32) {
    files.sort_by_cached_key(|f| std::cmp::Reverse(boost(f)));
}

/// The calculator row for `query`, if it is an arithmetic expression.
pub fn calc_item(query: &str) -> Option<Item> {
    let v = calc::format(calc::evaluate(query)?);
    Some(Item {
        target: Target::Calc(v.clone()),
        section: Section::Calculator,
        title: format!("= {v}"),
        subtitle: query.trim().to_string(),
        icon: "accessories-calculator".into(),
        tag: "Calculator",
    })
}

/// Valid history rows, resolved lazily so a full home needs no further
/// filesystem checks.
fn recent_items<'a>(apps: &'a [App], recent: &'a [(Kind, String)]) -> impl Iterator<Item = Item> + 'a {
    recent.iter().filter_map(|(kind, key)| match kind {
        Kind::App => apps.iter().position(|a| a.id == *key).map(|i| app_item(apps, i)),
        Kind::File => {
            let path = uri::path_from_file_url(key)?;
            let meta = std::fs::symlink_metadata(&path).ok()?;
            let name = path.file_name()?.to_string_lossy().into_owned();
            let parent = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let is_dir = meta.is_dir();
            Some(file_item(&FileHit {
                id: key.clone(),
                icon: if is_dir { "folder" } else { "text-x-generic" }.into(),
                subtext: home_relative(&parent),
                name,
                path: Some(path),
                is_dir,
            }))
        }
    })
}

/// Valid history rows first, then available applications in catalog order.
pub fn home_items(apps: &[App], recent: &[(Kind, String)], limit: usize) -> Vec<Item> {
    let mut seen_apps = vec![false; apps.len()];
    let mut items: Vec<_> = recent_items(apps, recent)
        .filter(|item| match item.target {
            Target::App(i) => !std::mem::replace(&mut seen_apps[i], true),
            _ => true,
        })
        .take(limit)
        .collect();
    for (i, seen) in seen_apps.into_iter().enumerate() {
        if items.len() == limit {
            break;
        }
        if !seen {
            items.push(app_item(apps, i));
        }
    }
    items
}

fn home_relative(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && (p == h || p.starts_with(&format!("{h}/"))) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

/// The merged result list. Without files: applications by score. With
/// files: strong application matches, then files, then a few weak ones.
pub fn build_items(apps: &[App], app_hits: &[(usize, u32)], files: &[FileHit]) -> Vec<Item> {
    let any_strong = app_hits.first().is_some_and(|&(_, s)| s >= STRONG_APP_SCORE);
    let app_hits: Vec<(usize, u32)> =
        app_hits.iter().copied().filter(|&(_, s)| !any_strong || s >= SUBSEQUENCE_CEILING).collect();
    if files.is_empty() {
        return app_hits.iter().take(MAX_APPS_ALONE).map(|&(i, _)| app_item(apps, i)).collect();
    }
    let (strong, weak): (Vec<_>, Vec<_>) = app_hits.iter().partition(|&&(_, s)| s >= STRONG_APP_SCORE);
    let mut items: Vec<Item> = strong.iter().take(MAX_STRONG_APPS).map(|&&(i, _)| app_item(apps, i)).collect();
    items.extend(files.iter().map(file_item));
    items.extend(weak.iter().take(MAX_WEAK_APPS).map(|&&(i, _)| app_item(apps, i)));
    items
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Launch,
    Open,
    Reveal,
    CopyPath,
    CopyName,
    CopyResult,
    UseResult,
    ResetRanking,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Launch => "Launch",
            Action::Open => "Open",
            Action::Reveal => "Show in folder",
            Action::CopyPath => "Copy path",
            Action::CopyName => "Copy name",
            Action::CopyResult => "Copy result",
            Action::UseResult => "Use result",
            Action::ResetRanking => "Reset ranking",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Action::Launch | Action::Open | Action::CopyResult => "Enter",
            Action::Reveal | Action::UseResult => "Ctrl+Enter",
            Action::CopyPath => "Ctrl+Shift+C",
            Action::CopyName | Action::ResetRanking => "",
        }
    }
}

/// Actions for an item; the first is the primary (Enter) action.
pub fn actions_for(t: &Target) -> Vec<Action> {
    match t {
        Target::App(_) => vec![Action::Launch, Action::CopyName, Action::ResetRanking],
        Target::File(f) if f.path.is_some() => {
            vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName, Action::ResetRanking]
        }
        Target::File(_) => vec![Action::Open, Action::Reveal, Action::CopyName, Action::ResetRanking],
        Target::Calc(_) => vec![Action::CopyResult, Action::UseResult],
    }
}

/// Ctrl+Enter always uses the selected result's secondary action, including
/// while that action is hidden by an action-panel filter.
pub fn secondary_action(actions: &[Action]) -> Option<Action> {
    actions.iter().copied().find(|a| matches!(a, Action::Reveal | Action::UseResult))
}

/// Selection plus the window of rows currently drawn (the list is
/// virtual: only `visible` rows exist, so no scroll state is needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub selected: usize,
    pub first: usize,
    pub visible: usize,
}

/// Confirms that both presses of a widget-reported double click refer to
/// the same result, even when rows were reused after an asynchronous update.
#[derive(Default)]
pub struct ClickTracker {
    previous: Option<Target>,
    repeated: bool,
}

impl ClickTracker {
    pub fn press(&mut self, target: &Target) {
        self.repeated = self.previous.as_ref() == Some(target);
        self.previous = Some(target.clone());
    }

    pub fn double_click(&mut self, target: &Target) -> bool {
        let same_target = self.repeated && self.previous.as_ref() == Some(target);
        *self = Self::default();
        same_target
    }
}

impl Cursor {
    pub fn new(visible: usize) -> Cursor {
        Cursor { selected: 0, first: 0, visible: visible.max(1) }
    }

    pub fn reset(&mut self) {
        self.selected = 0;
        self.first = 0;
    }

    /// Moves by `delta` rows, clamped to `[0, len)`, keeping the selection
    /// inside the window.
    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.reset();
            return;
        }
        let max = len - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max as isize) as usize;
        self.scroll_into_view(len);
    }

    pub fn select(&mut self, index: usize, len: usize) {
        if index < len {
            self.selected = index;
            self.scroll_into_view(len);
        }
    }

    /// Selects a numbered slot in the currently drawn window. Missing slots
    /// leave the selection unchanged and must not activate its previous target.
    pub fn select_visible(&mut self, slot: usize, len: usize) -> bool {
        if slot >= NUMBERED_SLOTS {
            return false;
        }
        let Some(index) = self.window(len).nth(slot) else { return false };
        self.select(index, len);
        true
    }

    /// Resolves the clicked target against the current list, which may
    /// have changed since the row was drawn.
    pub fn select_target(&mut self, target: &Target, items: &[Item]) -> bool {
        let Some(index) = items.iter().position(|item| item.target == *target) else { return false };
        self.select(index, items.len());
        true
    }

    /// Selects the first row of the adjacent group, without wrapping.
    pub fn move_section(&mut self, forward: bool, items: &[Item]) {
        let Some(current) = items.get(self.selected) else {
            self.reset();
            return;
        };
        let next = if forward {
            (self.selected + 1..items.len()).find(|&i| items[i].section != current.section)
        } else {
            (0..self.selected)
                .rev()
                .skip_while(|&i| items[i].section == current.section)
                .find(|&i| i == 0 || items[i - 1].section != items[i].section)
        };
        if let Some(i) = next {
            self.select(i, items.len());
        }
    }

    fn scroll_into_view(&mut self, len: usize) {
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + self.visible {
            self.first = self.selected + 1 - self.visible;
        }
        self.first = self.first.min(len.saturating_sub(self.visible));
    }

    pub fn window(&self, len: usize) -> Range<usize> {
        let start = self.first.min(len);
        start..(start + self.visible).min(len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Up,
    Down,
    PreviousSection,
    NextSection,
    PageUp,
    PageDown,
    First,
    Last,
    /// Primary action of the selection (or of the highlighted action when
    /// the action panel is open).
    Activate,
    /// Primary action of a zero-based visible result or filtered action slot.
    ActivateVisible(usize),
    /// Secondary action (Ctrl+Enter).
    Secondary,
    ToggleActions,
    OpenActions,
    CopyPath,
    /// Close the panel, else clear the query, else hide.
    Back,
}

/// Number-row shortcuts use physical positions on layouts whose digits need
/// Shift (such as AZERTY). Logical digits also support numpads/unknown codes.
/// These commands must be captured before a text input sees printable text.
pub fn numbered_command(key: &Key, physical: Physical, mods: Modifiers) -> Option<Command> {
    if mods != Modifiers::CTRL {
        return None;
    }
    let slot = match physical {
        Physical::Code(Code::Digit1) => 0,
        Physical::Code(Code::Digit2) => 1,
        Physical::Code(Code::Digit3) => 2,
        Physical::Code(Code::Digit4) => 3,
        Physical::Code(Code::Digit5) => 4,
        Physical::Code(Code::Digit6) => 5,
        Physical::Code(Code::Digit7) => 6,
        Physical::Code(Code::Digit8) => 7,
        _ => match key.as_ref() {
            Key::Character("1") => 0,
            Key::Character("2") => 1,
            Key::Character("3") => 2,
            Key::Character("4") => 3,
            Key::Character("5") => 4,
            Key::Character("6") => 5,
            Key::Character("7") => 6,
            Key::Character("8") => 7,
            _ => return None,
        },
    };
    Some(Command::ActivateVisible(slot))
}

/// Maps a key press to a launcher command. Keys not listed here go to the
/// search field.
pub fn command_for(key: &Key, mods: Modifiers) -> Option<Command> {
    let ctrl = mods.control();
    match key {
        Key::Named(Named::ArrowUp) if ctrl => Some(Command::PreviousSection),
        Key::Named(Named::ArrowDown) if ctrl => Some(Command::NextSection),
        Key::Named(Named::ArrowUp) => Some(Command::Up),
        Key::Named(Named::ArrowDown) => Some(Command::Down),
        Key::Named(Named::Tab) if mods.shift() => Some(Command::Up),
        Key::Named(Named::Tab) => Some(Command::Down),
        Key::Named(Named::PageUp) => Some(Command::PageUp),
        Key::Named(Named::PageDown) => Some(Command::PageDown),
        Key::Named(Named::Home) if ctrl => Some(Command::First),
        Key::Named(Named::End) if ctrl => Some(Command::Last),
        Key::Named(Named::Enter) if ctrl => Some(Command::Secondary),
        Key::Named(Named::Enter) => Some(Command::Activate),
        Key::Named(Named::Escape) => Some(Command::Back),
        Key::Named(Named::ContextMenu) => Some(Command::OpenActions),
        Key::Named(Named::F10) if mods.shift() => Some(Command::OpenActions),
        Key::Character(c) if ctrl => match c.as_str() {
            "k" | "K" => Some(Command::ToggleActions),
            "n" | "N" | "j" | "J" => Some(Command::Down),
            "p" | "P" => Some(Command::Up),
            "c" | "C" if mods.shift() => Some(Command::CopyPath),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn file(name: &str, dir: bool) -> FileHit {
        FileHit {
            id: format!("file:///t/{name}"),
            path: Some(PathBuf::from(format!("/t/{name}"))),
            name: name.into(),
            icon: if dir { "folder".into() } else { "text-plain".into() },
            subtext: "/t".into(),
            is_dir: dir,
        }
    }

    fn apps() -> Vec<App> {
        ["Firefox", "Files", "Fonts", "Calculator", "Konsole", "Kate"]
            .iter()
            .map(|n| {
                let t = format!("[Desktop Entry]\nType=Application\nName={n}\nExec=x\n");
                crate::apps::parse(&format!("{n}.desktop"), Path::new("/a"), &t, &[], &[]).unwrap()
            })
            .collect()
    }

    #[test]
    fn strong_apps_above_files_weak_below() {
        let a = apps();
        let hits = match_apps(&a, "f", |_| 0);
        assert_eq!(hits.len(), 3, "Firefox, Files, Fonts");
        let files: Vec<_> = (0..3).map(|i| file(&format!("f{i}.txt"), i == 0)).collect();
        let items = build_items(&a, &hits, &files);
        let sections: Vec<_> = items.iter().map(|i| i.section).collect();
        assert_eq!(sections[..3], [Section::Applications; 3]);
        assert_eq!(sections[3..], [Section::Files; 3]);
        assert_eq!(items[3].tag, "Folder");

        let weak = match_apps(&a, "clc", |_| 0); // subsequence of "calculator" only
        assert!(weak[0].1 < STRONG_APP_SCORE);
        let items = build_items(&a, &weak, &files);
        assert_eq!(items.last().unwrap().title, "Calculator", "weak match goes after files");
        assert_eq!(items[0].section, Section::Files);

        let mixed = [(0, 900), (1, 150)];
        assert_eq!(build_items(&a, &mixed, &[]).len(), 1, "subsequence noise dropped beside a strong match");
        assert_eq!(build_items(&a, &[(1, 150)], &[]).len(), 1, "kept when it is all there is");

        let many: Vec<(usize, u32)> = (0..a.len()).map(|i| (i, 900)).collect();
        assert_eq!(build_items(&a, &many, &files).iter().filter(|i| i.section == Section::Applications).count(), 4);
        assert_eq!(build_items(&a, &many, &[]).len(), 6);
    }

    #[test]
    fn exact_name_ranks_first() {
        let a = apps();
        let hits = match_apps(&a, "kate", |_| 0);
        assert_eq!(a[hits[0].0].name, "Kate");
    }

    #[test]
    fn history_reorders_comparable_matches_only() {
        let a = apps();
        let fonts = |x: &App| if x.name == "Fonts" { crate::usage::MAX_BOOST } else { 0 };
        let hits = match_apps(&a, "f", fonts);
        assert_eq!(a[hits[0].0].name, "Fonts", "a used prefix match moves up");
        let hits = match_apps(&a, "files", fonts);
        assert_eq!(a[hits[0].0].name, "Files", "an exact name still wins");
        let calc_boost = |x: &App| if x.name == "Calculator" { 500 } else { 0 };
        assert_eq!(
            match_apps(&a, "clc", calc_boost)[0].1,
            match_apps(&a, "clc", |_| 0)[0].1,
            "subsequence not boosted"
        );

        let mut files: Vec<_> = ["a", "b", "c"].iter().map(|n| file(n, false)).collect();
        order_files(&mut files, |f| if f.name == "c" { 30 } else { 0 });
        let names: Vec<_> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["c", "a", "b"]);
    }

    #[test]
    fn resetting_history_restores_app_and_file_order() {
        let a = apps();
        let mut usage = crate::usage::Usage::disabled();
        usage.record(Kind::App, "Fonts.desktop", "f", 100);
        assert_eq!(a[match_apps(&a, "f", |app| usage.boost(Kind::App, &app.id, "f", 100))[0].0].name, "Fonts");
        usage.reset(Kind::App, "Fonts.desktop").unwrap();
        assert_eq!(a[match_apps(&a, "f", |app| usage.boost(Kind::App, &app.id, "f", 100))[0].0].name, "Files");

        let original: Vec<_> = ["a", "b", "c"].iter().map(|n| file(n, false)).collect();
        usage.record(Kind::File, &original[2].id, "f", 100);
        let mut ranked = original.clone();
        order_files(&mut ranked, |f| usage.boost(Kind::File, &f.id, "f", 100));
        assert_eq!(ranked[0].name, "c");
        usage.reset(Kind::File, &original[2].id).unwrap();
        let mut ranked = original.clone();
        order_files(&mut ranked, |f| usage.boost(Kind::File, &f.id, "f", 100));
        assert_eq!(ranked, original);
        assert!(usage.recent(100, 8).is_empty());
    }

    #[test]
    fn history_never_crosses_match_classes() {
        let mk = |n: &str, comment: &str| {
            let t = format!("[Desktop Entry]\nType=Application\nName={n}\nComment={comment}\nExec=x\n");
            crate::apps::parse(&format!("{n}.desktop"), Path::new("/a"), &t, &[], &[]).unwrap()
        };
        let max = crate::usage::MAX_BOOST;

        // A heavily used prefix match never passes an exact name, and a
        // used description-only match stays weak (below files).
        let b = vec![mk("Files Plus", ""), mk("Files", ""), mk("Viewer", "files")];
        let used = |x: &App| if x.name != "Files" { max } else { 0 };
        let hits = match_apps(&b, "files", used);
        assert_eq!(b[hits[0].0].name, "Files");
        let items = build_items(&b, &hits, &[file("files.txt", false)]);
        assert_eq!(items.last().unwrap().title, "Viewer", "description match stays below files");

        // A multi-word name typed out in full is exact too.
        let c = vec![mk("Visual Studio Code - Insiders", ""), mk("Visual Studio Code", "")];
        let used = |x: &App| if x.name.ends_with("Insiders") { max } else { 0 };
        for q in ["visual studio code", "Visual  Studio Code "] {
            let hits = match_apps(&c, q, used);
            assert_eq!(c[hits[0].0].name, "Visual Studio Code", "{q:?}");
            assert_eq!(hits[0].1, EXACT_SCORE);
        }
        assert!(match_apps(&c, "studio visual code", |_| 0)[0].1 < EXACT_SCORE, "word order matters for exact");
    }

    #[test]
    fn calculator_and_recent_rows() {
        let c = calc_item(" 2 + 3*4 ").unwrap();
        assert_eq!((c.title.as_str(), c.subtitle.as_str()), ("= 14", "2 + 3*4"));
        assert_eq!(actions_for(&c.target), [Action::CopyResult, Action::UseResult]);
        assert!(calc_item("report").is_none());
        assert_eq!(calc_item("= 42").unwrap().title, "= 42");
        assert!(calc_item("=").is_none());

        let a = apps();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x y.txt");
        std::fs::write(&f, "").unwrap();
        let url = format!("file://{}", f.display()).replace(' ', "%20");
        let recent = [
            (Kind::App, "Kate.desktop".to_string()),
            (Kind::File, url),
            (Kind::App, "gone.desktop".to_string()),
            (Kind::File, "file:///nonexistent/ferroseek-test".to_string()),
        ];
        let items: Vec<_> = recent_items(&a, &recent).collect();
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Kate", "x y.txt"]);
    }

    #[test]
    fn home_filters_stale_history_before_the_limit() {
        let a = apps();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("valid.txt");
        std::fs::write(&f, "").unwrap();
        let recent = [
            (Kind::App, "gone.desktop".to_string()),
            (Kind::File, format!("file://{}/gone.txt", dir.path().display())),
            (Kind::App, "Kate.desktop".to_string()),
            (Kind::File, format!("file://{}", f.display())),
        ];
        let items = home_items(&a, &recent, 2);
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Kate", "valid.txt"]);
    }

    #[test]
    fn home_without_history_suggests_available_apps_in_catalog_order() {
        let a = apps();
        let items = home_items(&a, &[], 3);
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Firefox", "Files", "Fonts"]);
        assert_eq!(home_items(&a, &[], usize::MAX).len(), a.len());
        assert!(home_items(&[], &[], 3).is_empty());
    }

    #[test]
    fn home_suggestions_do_not_duplicate_recent_apps_and_respect_zero_limit() {
        let a = apps();
        let recent = [(Kind::App, "Files.desktop".to_string()), (Kind::App, "Files.desktop".to_string())];
        let items = home_items(&a, &recent, 4);
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Files", "Firefox", "Fonts", "Calculator"]);
        assert!(home_items(&a, &recent, 0).is_empty());
    }

    #[test]
    fn cursor_window_follows_selection() {
        let mut c = Cursor::new(3);
        c.move_by(1, 10);
        assert_eq!((c.selected, c.window(10)), (1, 0..3));
        c.move_by(3, 10);
        assert_eq!((c.selected, c.window(10)), (4, 2..5));
        c.move_by(100, 10);
        assert_eq!((c.selected, c.window(10)), (9, 7..10));
        c.move_by(-8, 10);
        assert_eq!((c.selected, c.window(10)), (1, 1..4));
        c.move_by(-5, 10);
        assert_eq!((c.selected, c.window(10)), (0, 0..3));
        c.move_by(1, 0);
        assert_eq!((c.selected, c.window(0)), (0, 0..0));
        let mut c = Cursor::new(5);
        c.select(1, 2);
        assert_eq!((c.selected, c.window(2)), (1, 0..2));
        c.select(7, 2);
        assert_eq!(c.selected, 1, "out of range is ignored");
    }

    #[test]
    fn numbered_selection_tracks_the_drawn_window_and_ignores_missing_slots() {
        let mut c = Cursor::new(NUMBERED_SLOTS);
        c.move_by(7, 20);
        c.move_by(7, 20);
        assert_eq!(c.window(20), 7..15);
        assert!(c.select_visible(0, 20));
        assert_eq!(c.selected, 7);
        assert!(c.select_visible(7, 20));
        assert_eq!(c.selected, 14);
        c.move_by(20, 20);
        assert_eq!(c.window(20), 12..20);
        assert!(c.select_visible(0, 20));
        assert_eq!(c.selected, 12);
        let before = c;
        assert!(!c.select_visible(8, 20));
        assert!(!c.select_visible(usize::MAX, 20));
        assert!(!c.select_visible(0, 0));
        assert_eq!(c, before);

        c.reset();
        c.select(1, 2);
        let before = c;
        assert!(!c.select_visible(2, 2));
        assert_eq!(c, before);
        assert!(c.select_visible(0, 2));
        assert_eq!(c.selected, 0);
    }

    #[test]
    fn numbered_keys_require_only_control_and_leave_plain_digits_to_the_field() {
        for (slot, digit) in (1..=NUMBERED_SLOTS).enumerate() {
            let key = Key::Character(digit.to_string().into());
            assert_eq!(
                numbered_command(
                    &key,
                    Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
                    Modifiers::CTRL
                ),
                Some(Command::ActivateVisible(slot))
            );
            for mods in [
                Modifiers::empty(),
                Modifiers::ALT,
                Modifiers::LOGO,
                Modifiers::CTRL | Modifiers::ALT,
                Modifiers::CTRL | Modifiers::SHIFT,
                Modifiers::CTRL | Modifiers::LOGO,
            ] {
                assert_eq!(numbered_command(&key, Physical::Code(Code::Digit1), mods), None);
            }
        }
        for digit in ["0", "9", "12", "!"] {
            assert_eq!(
                numbered_command(&Key::Character(digit.into()), Physical::Code(Code::Digit9), Modifiers::CTRL),
                None
            );
        }
    }

    #[test]
    fn numbered_keys_follow_physical_positions_on_azerty() {
        for (slot, (symbol, code)) in [
            ("&", Code::Digit1),
            ("é", Code::Digit2),
            ("\"", Code::Digit3),
            ("'", Code::Digit4),
            ("(", Code::Digit5),
            ("-", Code::Digit6),
            ("è", Code::Digit7),
            ("_", Code::Digit8),
        ]
        .into_iter()
        .enumerate()
        {
            let key = Key::Character(symbol.into());
            assert_eq!(
                numbered_command(&key, Physical::Code(code), Modifiers::CTRL),
                Some(Command::ActivateVisible(slot))
            );
            assert_eq!(numbered_command(&key, Physical::Code(code), Modifiers::empty()), None);
            assert_eq!(numbered_command(&key, Physical::Code(code), Modifiers::CTRL | Modifiers::SHIFT), None);
        }
    }

    #[test]
    fn pointer_selection_follows_target_identity_after_results_change() {
        let a = apps();
        let files = [file("notes.txt", false), file("report.txt", false)];
        let mut items = build_items(&a, &[], &files);
        let target = items[1].target.clone();
        let mut cursor = Cursor::new(2);

        // A late result can move the row between drawing it and handling
        // its click. Resolve the original target, not its old row index.
        items.insert(0, calc_item("2+2").unwrap());
        assert!(cursor.select_target(&target, &items));
        assert_eq!(cursor.selected, 2);
        assert_eq!(items[cursor.selected].title, "report.txt");
        assert!(cursor.window(items.len()).contains(&cursor.selected));

        items.pop();
        cursor.select(0, items.len());
        assert!(!cursor.select_target(&target, &items), "removed targets cannot activate a different row");
        assert_eq!(cursor.selected, 0);
        assert!(!cursor.select_target(&target, &[]));
    }

    #[test]
    fn double_click_rejects_a_row_reused_for_another_result() {
        let first = Target::File(file("first.txt", false));
        let second = Target::File(file("second.txt", false));
        let mut clicks = ClickTracker::default();
        clicks.press(&first);
        // The second press hits the same widget position after rows move.
        clicks.press(&second);
        assert!(!clicks.double_click(&second));

        clicks.press(&second);
        clicks.press(&second);
        assert!(clicks.double_click(&second));
        assert!(!clicks.double_click(&second), "one gesture activates at most once");

        clicks.press(&first);
        assert!(!clicks.double_click(&first), "a single press is not an activation");
    }

    #[test]
    fn context_menu_keys_open_actions_without_toggling_them() {
        assert_eq!(command_for(&Key::Named(Named::ContextMenu), Modifiers::empty()), Some(Command::OpenActions));
        assert_eq!(command_for(&Key::Named(Named::F10), Modifiers::SHIFT), Some(Command::OpenActions));
        assert_eq!(command_for(&Key::Named(Named::F10), Modifiers::empty()), None);
    }

    #[test]
    fn actions_put_primary_first() {
        assert_eq!(actions_for(&Target::App(0)), [Action::Launch, Action::CopyName, Action::ResetRanking]);
        let f = actions_for(&Target::File(file("a", false)));
        assert_eq!(f, [Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName, Action::ResetRanking]);
        let mut odd = file("a", false);
        odd.path = None;
        assert!(!actions_for(&Target::File(odd)).contains(&Action::CopyPath));
        let calculator = actions_for(&Target::Calc("2".into()));
        assert_eq!(calculator, [Action::CopyResult, Action::UseResult]);
        assert_eq!(secondary_action(&calculator), Some(Action::UseResult));
        assert_eq!(secondary_action(&f), Some(Action::Reveal));
        assert_eq!(secondary_action(&actions_for(&Target::App(0))), None);
    }

    #[test]
    fn section_navigation_skips_groups_and_keeps_the_target_visible() {
        let a = apps();
        let files: Vec<_> = (0..12).map(|i| file(&format!("f{i}.txt"), false)).collect();
        let mut items: Vec<_> = calc_item("2+2").into_iter().collect();
        items.extend(build_items(&a, &[(0, 900), (1, 800), (3, 450)], &files));
        let mut c = Cursor::new(3);
        for expected in [1, 3, 15, 15] {
            c.move_section(true, &items);
            assert_eq!(c.selected, expected);
            assert!(c.window(items.len()).contains(&expected));
        }
        for expected in [3, 1, 0, 0] {
            c.move_section(false, &items);
            assert_eq!(c.selected, expected);
            assert!(c.window(items.len()).contains(&expected));
        }
        c.select(8, items.len());
        c.move_section(false, &items);
        assert_eq!(c.selected, 1, "jump to the preceding group's first row from mid-group");
        c.select(8, items.len());
        c.move_section(true, &items);
        assert_eq!(c.selected, 15);

        let only_files = build_items(&a, &[], &files);
        c.select(5, only_files.len());
        c.move_section(false, &only_files);
        c.move_section(true, &only_files);
        assert_eq!(c.selected, 5, "a single group has no adjacent group");
        c.move_section(true, &[]);
        assert_eq!((c.selected, c.first), (0, 0));
    }

    #[test]
    fn key_map() {
        let none = Modifiers::empty();
        let k = |s: &str| Key::Character(s.into());
        assert_eq!(command_for(&Key::Named(Named::ArrowDown), none), Some(Command::Down));
        assert_eq!(command_for(&Key::Named(Named::ArrowDown), Modifiers::CTRL), Some(Command::NextSection));
        assert_eq!(command_for(&Key::Named(Named::ArrowUp), Modifiers::CTRL), Some(Command::PreviousSection));
        assert_eq!(command_for(&Key::Named(Named::Enter), none), Some(Command::Activate));
        assert_eq!(command_for(&Key::Named(Named::Enter), Modifiers::CTRL), Some(Command::Secondary));
        assert_eq!(command_for(&Key::Named(Named::Tab), Modifiers::SHIFT), Some(Command::Up));
        assert_eq!(command_for(&k("k"), Modifiers::CTRL), Some(Command::ToggleActions));
        assert_eq!(command_for(&k("C"), Modifiers::CTRL | Modifiers::SHIFT), Some(Command::CopyPath));
        assert_eq!(command_for(&k("c"), Modifiers::CTRL), None, "plain Ctrl+C copies field text");
        assert_eq!(command_for(&k("a"), none), None);
        assert_eq!(command_for(&Key::Named(Named::Home), none), None, "Home moves the text cursor");
    }
}
