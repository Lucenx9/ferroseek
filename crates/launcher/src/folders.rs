// SPDX-License-Identifier: GPL-3.0-or-later
//! Folder configuration view; writes and daemon requests run outside the UI thread.

use std::path::PathBuf;

use ferroseek_config::{
    Config, expand,
    folders::{Document, validate_folder},
    normalize,
};
use iced::alignment::Vertical;
use iced::widget::{self, Id, button, column, container, mouse_area, row, scrollable, space, text, text_input};
use iced::{Element, Length, Task};

use crate::backend::{Backend, FolderPolicy};
use crate::model::{Command, Cursor, home_relative};
use crate::ui::{self, palette};

#[derive(Debug, Clone)]
pub enum Event {
    Loaded { revision: u64, result: Result<Document, String> },
    Policy { revision: u64, result: Result<Option<FolderPolicy>, String> },
    Input(String),
    Add,
    Validated { path: String, result: Result<(), String> },
    Select(String),
    Remove(String),
    RemoveSelected,
    Save,
    Saved(Result<Document, String>),
    Applied(Result<bool, String>),
    Reload,
    Back,
    Discard,
    KeepEditing,
}

#[derive(Debug, Clone, Copy)]
pub enum Shortcut {
    Save,
    Remove,
}

#[derive(Clone, Copy)]
enum Pending {
    Close,
    Reload,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Loading,
    Ready,
    Validating,
    Saving,
    Applying,
}

pub enum Effect {
    None,
    Close { refresh: bool },
    Applied,
}

pub struct Panel {
    pub exit_on_cancel: bool,
    path: PathBuf,
    document: Option<Document>,
    roots: Vec<String>,
    input: String,
    cursor: Cursor,
    phase: Phase,
    message: Option<(String, bool)>,
    policy: Option<Config>,
    policy_warning: Option<String>,
    pending: Option<Pending>,
    save_attempted: bool,
    revision: u64,
}

/// Folder rows show at most this many characters; longer paths repeat in full below.
const ROW_PATH: usize = 40;

pub fn input_id() -> Id {
    Id::new("folder-path")
}
fn scroll_id() -> Id {
    Id::new("folder-settings-scroll")
}

impl Panel {
    pub fn new(path: PathBuf, exit_on_cancel: bool) -> Self {
        Self {
            path,
            exit_on_cancel,
            document: None,
            roots: Vec::new(),
            input: String::new(),
            cursor: Cursor::new(4),
            phase: Phase::Loading,
            message: None,
            policy: None,
            policy_warning: None,
            pending: None,
            save_attempted: false,
            revision: 0,
        }
    }

    pub fn load(&mut self, backend: Backend) -> Task<Event> {
        self.revision += 1;
        let revision = self.revision;
        let path = self.path.clone();
        Task::batch([
            Task::perform(async move { Document::load(path) }, move |result| Event::Loaded { revision, result }),
            Task::perform(backend.folder_policy(), move |result| Event::Policy { revision, result }),
        ])
    }

    fn dirty(&self) -> bool {
        self.document.as_ref().is_some_and(|doc| doc.config.roots != self.roots) || !self.input.is_empty()
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Validating | Phase::Saving | Phase::Applying)
    }

    fn reload(&mut self, backend: Backend) -> Task<Event> {
        self.phase = Phase::Loading;
        self.document = None;
        self.roots.clear();
        self.input.clear();
        self.cursor.reset();
        self.message = None;
        self.load(backend)
    }

    pub fn update(&mut self, event: Event, backend: Backend) -> (Task<Event>, Effect) {
        let mut task = Task::none();
        match event {
            Event::Policy { revision, result } if revision == self.revision => match result {
                Ok(Some(policy)) => {
                    self.policy = Some(policy.config);
                    self.policy_warning = (std::fs::canonicalize(&self.path).ok()
                        != std::fs::canonicalize(&policy.config_path).ok())
                    .then(|| "The running service uses a different configuration. Folders can be saved here, but cannot be applied to that service.".into());
                }
                // Without a reachable service the summary falls back to the saved file.
                Ok(None) | Err(_) => {
                    self.policy = None;
                    self.policy_warning = None;
                }
            },
            Event::Loaded { revision, result } if revision == self.revision && self.phase == Phase::Loading => {
                match result {
                    Ok(doc) => {
                        self.roots = doc.config.roots.clone();
                        self.input.clear();
                        self.cursor.reset();
                        self.document = Some(doc);
                        self.phase = Phase::Ready;
                        task = crate::input::focus(input_id());
                    }
                    Err(error) => {
                        self.phase = Phase::Ready;
                        self.message = Some((error, true));
                    }
                }
            }
            Event::Input(input) if self.phase == Phase::Ready && self.pending.is_none() => {
                self.input = input;
                self.message = None;
            }
            Event::Add if self.phase == Phase::Ready && self.pending.is_none() && self.document.is_some() => {
                if self.input.is_empty() {
                    self.message = Some(("Enter a folder path first.".into(), true));
                } else {
                    let path = self.input.clone();
                    self.phase = Phase::Validating;
                    task = Task::perform(
                        async move {
                            let result = validate_folder(&path);
                            Event::Validated { path, result }
                        },
                        |event| event,
                    );
                }
            }
            Event::Validated { path, result } if self.phase == Phase::Validating => {
                self.phase = Phase::Ready;
                match result {
                    Ok(()) => {
                        let new = normalize(&expand(&path));
                        let mut covered = None;
                        let mut duplicate = false;
                        for root in &self.roots {
                            let old = normalize(&expand(root));
                            if old == new {
                                duplicate = true;
                            } else if new.starts_with(&old) {
                                covered = Some(format!("Added. {root} already covers this folder."));
                            } else if covered.is_none() && old.starts_with(&new) {
                                covered = Some(format!("Added. {root} is inside it and can be removed."));
                            }
                        }
                        if duplicate {
                            self.message = Some(("This folder is already selected.".into(), false));
                        } else {
                            self.message = covered.map(|text| (text, false));
                            self.roots.push(path);
                            self.cursor.select(self.roots.len() - 1, self.roots.len());
                        }
                        self.input.clear();
                    }
                    Err(error) => self.message = Some((error, true)),
                }
                task = crate::input::focus(input_id());
            }
            Event::Select(path) if self.phase == Phase::Ready => {
                if let Some(i) = self.roots.iter().position(|r| r == &path) {
                    self.cursor.select(i, self.roots.len());
                }
            }
            Event::Remove(path) if self.phase == Phase::Ready && self.pending.is_none() => {
                if let Some(i) = self.roots.iter().position(|r| r == &path) {
                    self.roots.remove(i);
                    self.cursor.select(i.min(self.roots.len().saturating_sub(1)), self.roots.len());
                    self.message = None;
                }
            }
            Event::RemoveSelected => {
                if let Some(path) = self.roots.get(self.cursor.selected).cloned() {
                    return self.update(Event::Remove(path), backend);
                }
            }
            Event::Save if self.phase == Phase::Ready && self.pending.is_none() => {
                if !self.input.is_empty() {
                    self.message =
                        Some(("Add the typed folder with Enter, or clear the input before saving.".into(), true));
                } else if let Some(doc) = &self.document {
                    let doc = doc.clone();
                    let roots = self.roots.clone();
                    self.phase = Phase::Saving;
                    self.message = None;
                    self.save_attempted = true;
                    task = Task::perform(async move { doc.save(roots) }, Event::Saved);
                }
            }
            Event::Saved(result) if self.phase == Phase::Saving => match result {
                Ok(doc) => {
                    let path = doc.path.to_string_lossy().into_owned();
                    let roots = doc.config.roots.clone();
                    self.document = Some(doc);
                    self.phase = Phase::Applying;
                    task = Task::perform(backend.apply_folders(path, roots), Event::Applied);
                }
                Err(error) => {
                    self.phase = Phase::Ready;
                    self.message = Some((error, true));
                }
            },
            Event::Applied(result) if self.phase == Phase::Applying => {
                self.phase = Phase::Ready;
                match result {
                    Ok(other_pending) => {
                        if other_pending {
                            self.message = Some(("Folders applied. Other saved settings differ from the active service and need a service restart.".into(), false));
                            self.revision += 1;
                            let revision = self.revision;
                            task = Task::perform(backend.folder_policy(), move |result| Event::Policy {
                                revision,
                                result,
                            });
                        } else {
                            return (Task::none(), Effect::Applied);
                        }
                    }
                    Err(error) => {
                        self.message =
                            Some((format!("Folders saved, but application was not confirmed: {error}"), true))
                    }
                }
            }
            Event::Reload if self.phase == Phase::Loading => {}
            Event::Back | Event::Reload if !self.busy() => {
                let action = if matches!(event, Event::Back) { Pending::Close } else { Pending::Reload };
                if self.pending.is_some() {
                    self.pending = None;
                } else if self.dirty() {
                    self.pending = Some(action);
                    task = widget::operation::snap_to(scroll_id(), widget::operation::RelativeOffset::END);
                } else {
                    match action {
                        Pending::Close => return (Task::none(), Effect::Close { refresh: self.save_attempted }),
                        Pending::Reload => task = self.reload(backend),
                    }
                }
            }
            Event::Discard if !self.busy() => {
                if let Some(action) = self.pending.take() {
                    match action {
                        Pending::Close => return (Task::none(), Effect::Close { refresh: self.save_attempted }),
                        Pending::Reload => task = self.reload(backend),
                    }
                }
            }
            Event::KeepEditing => {
                self.pending = None;
                task = Task::batch([
                    crate::input::focus(input_id()),
                    widget::operation::snap_to(scroll_id(), widget::operation::RelativeOffset::START),
                ]);
            }
            _ => {}
        }
        if self.pending.is_none() && (matches!(self.phase, Phase::Saving | Phase::Applying) || self.message.is_some()) {
            task =
                Task::batch([task, widget::operation::snap_to(scroll_id(), widget::operation::RelativeOffset::START)]);
        }
        (task, Effect::None)
    }

    pub fn command(&mut self, cmd: Command, backend: Backend) -> (Task<Event>, Effect) {
        match cmd {
            Command::Back => return self.update(Event::Back, backend),
            Command::Activate => return self.update(Event::Add, backend),
            Command::Up | Command::Down if !self.busy() => {
                self.cursor.move_by(if cmd == Command::Up { -1 } else { 1 }, self.roots.len());
                return (
                    widget::operation::snap_to(scroll_id(), widget::operation::RelativeOffset::START),
                    Effect::None,
                );
            }
            Command::PageUp | Command::PageDown => {
                return (
                    widget::operation::scroll_by(
                        scroll_id(),
                        widget::operation::AbsoluteOffset {
                            x: 0.0,
                            y: if cmd == Command::PageUp { -180.0 } else { 180.0 },
                        },
                    ),
                    Effect::None,
                );
            }
            Command::First => {
                return (
                    widget::operation::snap_to(scroll_id(), widget::operation::RelativeOffset::START),
                    Effect::None,
                );
            }
            Command::Last => {
                return (widget::operation::snap_to(scroll_id(), widget::operation::RelativeOffset::END), Effect::None);
            }
            _ => {}
        }
        (Task::none(), Effect::None)
    }

    pub fn shortcut(&mut self, shortcut: Shortcut, backend: Backend) -> (Task<Event>, Effect) {
        let event = match shortcut {
            Shortcut::Save => Event::Save,
            Shortcut::Remove if self.pending.is_some() => Event::Discard,
            Shortcut::Remove => Event::RemoveSelected,
        };
        self.update(event, backend)
    }

    pub fn view(&self) -> Element<'_, Event> {
        let editable = self.phase == Phase::Ready && self.pending.is_none() && self.document.is_some();
        let mut reload = button(ui::hint("Ctrl+R", "Reload")).padding([6, 8]).style(ui::status_button_style);
        if !self.busy() && self.phase != Phase::Loading {
            reload = reload.on_press(Event::Reload);
        }
        let header =
            container(row![text("Search folders").size(20), space::horizontal(), reload].align_y(Vertical::Center))
                .height(58)
                .padding(iced::padding::left(16).right(8))
                .align_y(Vertical::Center);
        let mut body = column![].spacing(10).padding(16);
        if self.phase == Phase::Loading {
            body = body.push(text("Loading folder settings…").size(16));
        } else {
            if self.exit_on_cancel {
                body = body.push(text("Review your folders, then save to start file search.").size(14));
            }
            if let Some((message, error)) = &self.message {
                body = body.push(text(message).size(13).color(if *error { palette::ERROR } else { palette::ACCENT }));
            }
            if self.busy() {
                body = body.push(
                    text(match self.phase {
                        Phase::Saving => "Saving folders…",
                        Phase::Applying => "Applying folders…",
                        _ => "Checking folder…",
                    })
                    .size(13)
                    .color(palette::ACCENT),
                );
            }
            let mut input = text_input("Absolute path or ~/Documents", &self.input)
                .id(input_id())
                .size(14)
                .padding(8)
                .style(ui::field_style);
            let mut add = button(ui::hint("Enter", "Add folder")).style(ui::status_button_style);
            if editable {
                input = input.on_input(Event::Input);
                add = add.on_press(Event::Add);
            }
            body = body.push(row![input, add].spacing(8).align_y(Vertical::Center));
            if self.roots.is_empty() {
                body = body.push(
                    text("No folders selected. File search will have no indexed files.").size(13).color(palette::DIM),
                );
            }
            for i in self.cursor.window(self.roots.len()) {
                let path = &self.roots[i];
                let selected = self.cursor.selected == i;
                let mut remove = button(text("Remove").size(12)).style(ui::status_button_style);
                if editable {
                    remove = remove.on_press(Event::Remove(path.clone()));
                }
                let content = container(
                    row![text(ui::ellipsize_middle(path, ROW_PATH)).size(14), space::horizontal(), remove]
                        .align_y(Vertical::Center),
                )
                .padding([4, 8])
                .style(move |_| container::Style {
                    background: selected.then_some(iced::Background::Color(palette::SELECTED)),
                    border: iced::Border { radius: 5.0.into(), ..Default::default() },
                    ..Default::default()
                });
                body = body.push(mouse_area(content).on_press(Event::Select(path.clone())));
            }
            if let Some(path) = self.roots.get(self.cursor.selected).filter(|path| path.chars().count() > ROW_PATH) {
                body = body.push(text(path).size(12).color(palette::DIM).wrapping(text::Wrapping::WordOrGlyph));
            }
            if self.roots.len() > 4 {
                let window = self.cursor.window(self.roots.len());
                body = body.push(
                    text(format!("{}–{} / {} folders", window.start + 1, window.end, self.roots.len()))
                        .size(12)
                        .color(palette::DIM),
                );
            }
            if !self.roots.is_empty() {
                body = body.push(text("↑↓ select · Ctrl+D remove").size(12).color(palette::DIM));
            }
            if let Some(warning) = &self.policy_warning {
                body = body.push(text(warning).size(12).color(palette::ERROR).wrapping(text::Wrapping::WordOrGlyph));
            }
            let config = self.policy.as_ref().or_else(|| self.document.as_ref().map(|doc| &doc.config));
            if let Some(config) = config {
                let file = home_relative(&self.path.to_string_lossy());
                body = body.push(
                    text(format!("{} Edit {file} to change this.", exclusion_summary(config)))
                        .size(12)
                        .color(palette::DIM)
                        .wrapping(text::Wrapping::WordOrGlyph),
                );
            }
        }
        if self.pending.is_some() {
            body = body.push(text("Discard unsaved folder changes?").size(14).color(palette::ACCENT));
            body = body.push(
                row![
                    button(text("Keep editing").size(13)).on_press(Event::KeepEditing).style(ui::status_button_style),
                    button(ui::hint("Ctrl+D", "Discard changes"))
                        .on_press(Event::Discard)
                        .style(ui::status_button_style)
                ]
                .spacing(12),
            );
        }
        let mut save = button(ui::hint("Ctrl+S", "Save & apply")).padding([4, 8]).style(ui::status_button_style);
        let mut back = button(ui::hint("Esc", if self.exit_on_cancel { "Close" } else { "Back" }))
            .padding([4, 8])
            .style(ui::status_button_style);
        if self.phase == Phase::Ready && self.document.is_some() && self.pending.is_none() {
            save = save.on_press(Event::Save);
        }
        if !self.busy() {
            back = back.on_press(Event::Back);
        }
        let footer = container(row![back, space::horizontal(), save].align_y(Vertical::Center))
            .height(34)
            .padding([0, 8])
            .align_y(Vertical::Center);
        let body = scrollable(body).id(scroll_id()).height(Length::Fill);
        container(column![header, ui::divider(), body, ui::divider(), footer])
            .padding(8)
            .height(Length::Fill)
            .width(Length::Fill)
            .style(ui::surface_style)
            .into()
    }
}

/// What indexing skips, in one sentence; the configuration file holds the lists.
fn exclusion_summary(config: &Config) -> String {
    let count = |n: usize, what: &str| format!("{n} excluded {what}{}", if n == 1 { "" } else { "s" });
    let mut skipped = Vec::new();
    if !config.index_hidden {
        skipped.push("hidden files".to_string());
    }
    if config.respect_cachedir_tag {
        skipped.push("cache folders".into());
    }
    if !config.exclude_names.is_empty() {
        skipped.push(count(config.exclude_names.len(), "name"));
    }
    if !config.exclude_paths.is_empty() {
        skipped.push(count(config.exclude_paths.len(), "folder"));
    }
    let verb = if skipped.len() == 1 && skipped[0].starts_with("1 ") { "is" } else { "are" };
    let mut sentence = match skipped.split_last() {
        None => return "Nothing is excluded.".into(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    };
    sentence[..1].make_ascii_uppercase();
    format!("{sentence} {verb} skipped.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> (tempfile::TempDir, Panel) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "roots = []\nexclude_names = ['private*']\n").unwrap();
        let mut panel = Panel::new(path.clone(), false);
        let _ = panel.load(Backend::disconnected());
        let revision = panel.revision;
        let _ = panel.update(Event::Loaded { revision, result: Document::load(path) }, Backend::disconnected());
        (temp, panel)
    }

    #[test]
    fn dirty_edits_require_explicit_discard_and_root_clicks_keep_identity() {
        let (_temp, mut panel) = loaded();
        let backend = Backend::disconnected();
        let _ = panel.update(Event::Input("/synthetic/new folder".into()), backend.clone());
        let _ = panel.update(Event::Add, backend.clone());
        let _ =
            panel.update(Event::Validated { path: "/synthetic/new folder".into(), result: Ok(()) }, backend.clone());
        assert_eq!(panel.roots, ["/synthetic/new folder"]);
        let (_, effect) = panel.update(Event::Back, backend.clone());
        assert!(matches!(effect, Effect::None));
        assert!(panel.pending.is_some());
        let _ = panel.update(Event::Save, backend.clone());
        assert!(panel.phase == Phase::Ready, "discard prompt must not permit a write");
        let _ = panel.update(Event::KeepEditing, backend.clone());
        let _ = panel.update(Event::Remove("/no-longer-listed".into()), backend.clone());
        assert_eq!(panel.roots.len(), 1);
        let _ = panel.update(Event::Reload, backend.clone());
        assert!(panel.pending.is_some());
        let _ = panel.update(Event::Back, backend.clone());
        assert!(panel.pending.is_none(), "Escape keeps editing a dirty draft");
        let _ = panel.update(Event::Back, backend.clone());
        let (_, effect) = panel.shortcut(Shortcut::Remove, backend);
        assert!(matches!(effect, Effect::Close { refresh: false }));
    }

    #[test]
    fn nested_additions_explain_coverage_and_exclusions_fit_one_sentence() {
        let (_temp, mut panel) = loaded();
        let backend = Backend::disconnected();
        let add = |panel: &mut Panel, path: &str| {
            let _ = panel.update(Event::Input(path.into()), backend.clone());
            let _ = panel.update(Event::Add, backend.clone());
            let _ = panel.update(Event::Validated { path: path.into(), result: Ok(()) }, backend.clone());
            panel.message.clone().map(|(text, _)| text)
        };
        assert_eq!(add(&mut panel, "/synthetic/work/notes"), None);
        assert_eq!(
            add(&mut panel, "/synthetic/work/notes/a"),
            Some("Added. /synthetic/work/notes already covers this folder.".into())
        );
        assert_eq!(
            add(&mut panel, "/synthetic/work"),
            Some("Added. /synthetic/work/notes is inside it and can be removed.".into())
        );
        assert_eq!(add(&mut panel, "/synthetic/work/"), Some("This folder is already selected.".into()));
        assert_eq!(add(&mut panel, "/synthetic/workshop"), None, "a shared name prefix is not nesting");
        assert_eq!(panel.roots.len(), 4);

        let config = &panel.document.as_ref().unwrap().config;
        assert_eq!(exclusion_summary(config), "Hidden files, cache folders and 1 excluded name are skipped.");
        let config = Config {
            index_hidden: true,
            respect_cachedir_tag: false,
            exclude_names: vec![],
            exclude_paths: vec!["/synthetic/a".into(), "/synthetic/b".into()],
            ..Config::default()
        };
        assert_eq!(exclusion_summary(&config), "2 excluded folders are skipped.");
        let config = Config { exclude_paths: vec!["/synthetic/a".into()], ..config };
        assert_eq!(exclusion_summary(&config), "1 excluded folder is skipped.");
        let config = Config { exclude_paths: vec![], ..config };
        assert_eq!(exclusion_summary(&config), "Nothing is excluded.");
    }

    #[test]
    fn saving_preserves_the_document_when_application_fails_and_can_be_retried() {
        let (temp, mut panel) = loaded();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        panel.roots.push(root.to_str().unwrap().into());
        let backend = Backend::disconnected();
        let _ = panel.shortcut(Shortcut::Save, backend.clone());
        assert!(panel.phase == Phase::Saving);
        let saved = panel.document.as_ref().unwrap().save(panel.roots.clone()).unwrap();
        let _ = panel.update(Event::Saved(Ok(saved.clone())), backend.clone());
        assert!(panel.phase == Phase::Applying);
        let _ = panel.update(Event::Back, backend.clone());
        assert!(panel.pending.is_none(), "a dispatched save/apply is not discarded halfway");
        let _ = panel.update(Event::Applied(Err("confirmation lost".into())), backend.clone());
        assert!(panel.phase == Phase::Ready);
        assert!(!panel.dirty());
        assert_eq!(panel.document.as_ref().unwrap().config, saved.config);
        assert!(panel.message.as_ref().unwrap().0.contains("saved"));
        let _ = panel.update(Event::Save, backend.clone());
        let _ = panel.update(Event::Saved(Ok(saved)), backend.clone());
        let (_, effect) = panel.update(Event::Applied(Ok(false)), backend);
        assert!(matches!(effect, Effect::Applied));
    }

    #[test]
    fn stale_loads_cannot_replace_a_newer_draft_and_empty_scope_is_supported() {
        let (_temp, mut panel) = loaded();
        let backend = Backend::disconnected();
        let old = panel.revision;
        let _ = panel.update(Event::Reload, backend.clone());
        assert!(panel.revision > old);
        let _ = panel.update(Event::Loaded { revision: old, result: Err("old failure".into()) }, backend.clone());
        assert!(panel.phase == Phase::Loading);
        assert!(panel.message.is_none());
        let revision = panel.revision;
        let doc = Document::load(panel.path.clone()).unwrap();
        let _ = panel.update(Event::Loaded { revision, result: Ok(doc) }, backend.clone());
        let _ = panel.update(Event::Save, backend.clone());
        assert!(panel.phase == Phase::Saving, "zero folders is an explicit valid choice");
        let _ = panel.update(Event::Saved(Err("changed on disk".into())), backend);
        assert!(panel.phase == Phase::Ready);
        assert!(panel.roots.is_empty());
        assert!(panel.message.as_ref().unwrap().1);
    }
}
