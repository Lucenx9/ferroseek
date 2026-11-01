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
                    self.policy_note = format!("Active exclusions · {}", policy.config_path);
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
        let header = container(
            row![
                text("Search folders").size(20),
                space::horizontal(),
                button(ui::hint("Ctrl+R", "Reload")).on_press(Event::Reload).style(ui::status_button_style)
            ]
            .align_y(Vertical::Center),
        )
        .height(58)
        .padding([0, 16])
        .align_y(Vertical::Center);
        let mut body = column![].spacing(10).padding(16);
        if self.phase == Phase::Loading {
            body = body.push(text("Loading folder settings…").size(16));
        } else {
            body = body.push(
                text(if self.exit_on_cancel {
                    "Choose folders before file indexing starts."
                } else {
                    "Choose where FerroSeek searches for files."
                })
                .size(14),
            );
            let input = text_input("Absolute path or ~/Documents", &self.input).id(input_id()).size(14).padding(8);
            body = body.push(
                row![
                    input.on_input(Event::Input),
                    button(ui::hint("Enter", "Add folder")).on_press(Event::Add).style(ui::status_button_style)
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            );
            if self.roots.is_empty() {
                body = body.push(
                    text("No folders selected. File search will have no indexed files.").size(13).color(palette::DIM),
                );
            }
            for i in self.cursor.window(self.roots.len()) {
                let path = &self.roots[i];
                let selected = self.cursor.selected == i;
                let content = container(
                    row![
                        text(ui::ellipsize_middle(path, 40)).size(14),
                        space::horizontal(),
                        button(text("Remove").size(12))
                            .on_press(Event::Remove(path.clone()))
                            .style(ui::status_button_style)
                    ]
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
        let mut save = button(ui::hint("Ctrl+S", "Save & apply")).style(ui::status_button_style);
        let mut back =
            button(ui::hint("Esc", if self.exit_on_cancel { "Close" } else { "Back" })).style(ui::status_button_style);
        if self.phase == Phase::Ready && self.document.is_some() && self.pending.is_none() {
            save = save.on_press(Event::Save);
        }
        if !self.busy() {
            back = back.on_press(Event::Back);
        }
        let footer = container(row![back, space::horizontal(), save].align_y(Vertical::Center))
            .height(34)
            .padding([0, 10])
            .align_y(Vertical::Center);
        container(column![header, scrollable(body).id(scroll_id()).height(Length::Fill), footer])
            .padding(8)
            .height(Length::Fill)
            .width(Length::Fill)
            .style(ui::surface_style)
            .into()
    }
}
