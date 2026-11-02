// SPDX-License-Identifier: GPL-3.0-or-later
//! Folder configuration view; writes and daemon requests run outside the UI thread.

use std::path::PathBuf;

use ferroseek_config::{
    Config,
    folders::{Document, validate_folder},
};
use iced::alignment::Vertical;
use iced::widget::{self, Id, button, column, container, mouse_area, row, scrollable, space, text, text_input};
use iced::{Element, Length, Task};

use crate::backend::{Backend, FolderPolicy};
use crate::model::{Command, Cursor};
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
    policy_note: String,
    pending: Option<Pending>,
    save_attempted: bool,
    revision: u64,
}

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
            policy_note: "Checking active exclusions…".into(),
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
                    self.policy_note = if std::fs::canonicalize(&self.path).ok()
                        == std::fs::canonicalize(&policy.config_path).ok()
                    {
                        "Active exclusion settings:".into()
                    } else {
                        "The running service uses a different configuration. Folders can be saved here, but cannot be applied to that service.".into()
                    };
                }
                Ok(None) => {
                    self.policy = None;
                    self.policy_note = "Service is not running. Saved exclusions will be used when it starts.".into();
                }
                Err(error) => {
                    self.policy = None;
                    self.policy_note = format!("Active exclusions could not be checked: {error}");
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
                        task = widget::operation::focus(input_id());
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
                        if self.roots.contains(&path) {
                            self.message = Some(("This folder is already selected.".into(), false));
                        } else {
                            self.roots.push(path);
                            self.cursor.select(self.roots.len() - 1, self.roots.len());
                        }
                        self.input.clear();
                    }
                    Err(error) => self.message = Some((error, true)),
                }
                task = widget::operation::focus(input_id());
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
                    widget::operation::focus(input_id()),
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
            body = body.push(
                text(if self.exit_on_cancel {
                    "Review your folders, then save to start file search."
                } else {
                    "Choose where FerroSeek searches for files."
                })
                .size(14),
            );
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
                    row![text(ui::ellipsize_middle(path, 40)).size(14), space::horizontal(), remove]
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
            if let Some(path) = self.roots.get(self.cursor.selected) {
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
            body = body.push(
                text("↑↓ select · Ctrl+D remove selected · nested folders are covered by their parent")
                    .size(12)
                    .color(palette::DIM),
            );
            body = body.push(text("Exclusions").size(15));
            body =
                body.push(text(&self.policy_note).size(12).color(palette::DIM).wrapping(text::Wrapping::WordOrGlyph));
            let config = self.policy.as_ref().or_else(|| self.document.as_ref().map(|doc| &doc.config));
            if let Some(config) = config {
                if self.policy.is_none() {
                    body = body.push(text("Saved exclusion settings:").size(12).color(palette::DIM));
                }
                body = body.push(
                    text(format!(
                        "Hidden files: {} · Cache-tagged folders: {} · Stay on each filesystem: {} · Maximum depth: {}",
                        if config.index_hidden { "included" } else { "excluded" },
                        if config.respect_cachedir_tag { "excluded" } else { "included" },
                        if config.stay_on_filesystem { "yes" } else { "no" },
                        config.max_depth
                    ))
                    .size(12)
                    .color(palette::DIM),
                );
                body = body.push(
                    text(format!(
                        "Names: {}",
                        if config.exclude_names.is_empty() { "none".into() } else { config.exclude_names.join(", ") }
                    ))
                    .size(12)
                    .color(palette::DIM),
                );
                body = body.push(
                    text(format!(
                        "Excluded folders: {}",
                        if config.exclude_paths.is_empty() { "none".into() } else { config.exclude_paths.join(", ") }
                    ))
                    .size(12)
                    .color(palette::DIM),
                );
                body = body.push(
                    text("FerroSeek's own index folder is always excluded. This screen changes only search folders.")
                        .size(12)
                        .color(palette::DIM),
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
