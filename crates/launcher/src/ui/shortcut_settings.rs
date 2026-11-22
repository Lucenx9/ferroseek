// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit local bindings while leaving native text editing and recovery keys intact.

use std::path::PathBuf;

use ferroseek_config::folders::Document;
use ferroseek_config::shortcuts::{Action, Chord, Preferences};
use iced::widget::operation::{self, RelativeOffset};
use iced::widget::{button, column, container, keyed_column, row, scrollable, space, text};
use iced::{Element, Length, Task, Theme};

use super::{
    Launcher, Message, divider, footer_style, hint, keycap, page_header, palette, primary_hint, status_button_style,
    surface_style, tab_style, update_content,
};
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{ClickTracker, Command};
use crate::{folders, timing};

const SCROLL: &str = "shortcut-settings-scroll";
const RESET: usize = Action::ALL.len();

#[derive(Debug, Clone)]
pub enum Event {
    Select(usize),
    Loaded(Result<Document, String>),
    Saved(Result<Document, String>),
    Save,
    Back,
    Discard,
    Reload,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Busy,
    Editing,
    Recording,
    Discard,
}

pub(super) struct State {
    pub(super) bindings: Preferences,
    pub(super) panel: Option<Panel>,
    generation: u64,
}

impl State {
    pub(super) fn new(bindings: Preferences) -> Self {
        Self { bindings, panel: None, generation: 0 }
    }
}

pub(super) struct Panel {
    path: PathBuf,
    original: Preferences,
    draft: Preferences,
    document: Option<Document>,
    selected: usize,
    mode: Mode,
    notice: String,
    error: bool,
}

impl Panel {
    fn new(path: PathBuf, current: Preferences) -> Self {
        Self {
            path,
            original: current,
            draft: current,
            document: None,
            selected: 0,
            mode: Mode::Busy,
            notice: "Loading shortcuts…".into(),
            error: false,
        }
    }

    pub(super) fn recording(&self) -> bool {
        self.mode == Mode::Recording
    }

    fn load(&self) -> Task<Event> {
        let path = self.path.clone();
        Task::perform(async move { zruntime::unblock(move || Document::load(path)).await }, Event::Loaded)
    }

    fn status(&mut self, message: impl Into<String>, error: bool) {
        self.notice = message.into();
        self.error = error;
    }

    fn loaded(&mut self, result: Result<Document, String>) {
        self.mode = Mode::Editing;
        match result {
            Ok(document) => {
                let latest = document.config.shortcuts;
                if self.original != self.draft && latest != self.original {
                    self.status("Shortcuts changed on disk. Discard your draft and reopen this page.", true);
                    return;
                }
                if self.draft == self.original {
                    self.draft = latest;
                }
                self.original = latest;
                self.document = Some(document);
                self.status("", false);
                timing::event("shortcuts_ready");
            }
            Err(error) => self.status(error, true),
        }
    }

    fn saved(&mut self, result: Result<Document, String>) {
        self.mode = Mode::Editing;
        match result {
            Ok(document) => {
                self.original = document.config.shortcuts;
                self.document = Some(document);
                self.status("Shortcuts saved", false);
                timing::event("shortcuts_saved");
            }
            Err(error) => {
                self.status(error, true);
                timing::event("shortcuts_write_failed");
            }
        }
    }

    fn select(&mut self, slot: usize) -> Task<Event> {
        self.selected = slot.min(RESET);
        operation::snap_to(SCROLL, RelativeOffset { x: 0.0, y: self.selected as f32 / RESET as f32 })
    }

    fn start_edit(&mut self, slot: usize) -> Task<Event> {
        if self.document.is_none() || self.mode != Mode::Editing {
            return Task::none();
        }
        let scroll = self.select(slot);
        if self.selected == RESET {
            self.draft = Preferences::default();
            self.status("Defaults selected. Save to apply.", false);
        } else {
            self.mode = Mode::Recording;
            timing::event("shortcut_recording");
            self.status("Press the new shortcut. Esc cancels.", false);
        }
        scroll
    }

    fn record(&mut self, chord: Result<Chord, &'static str>) -> Task<Event> {
        if !self.recording() {
            return Task::none();
        }
        let action = Action::ALL[self.selected];
        match chord.map_err(str::to_owned).and_then(|chord| self.draft.set(action, chord)) {
            Ok(()) => {
                self.mode = Mode::Editing;
                self.status("Save to apply.", false);
                timing::event("shortcut_staged");
            }
            Err(error) => {
                self.status(error, true);
                timing::event("shortcut_rejected");
            }
        }
        Task::none()
    }

    fn save(&mut self) -> Task<Event> {
        if self.mode != Mode::Editing || self.draft == self.original {
            return Task::none();
        }
        let Some(document) = self.document.clone() else { return Task::none() };
        self.mode = Mode::Busy;
        self.status("Saving shortcuts…", false);
        let draft = self.draft;
        Task::perform(async move { zruntime::unblock(move || document.save_shortcuts(draft)).await }, Event::Saved)
    }

    fn reload(&mut self) -> Task<Event> {
        if self.mode != Mode::Editing {
            return Task::none();
        }
        self.mode = Mode::Busy;
        self.status("Loading shortcuts…", false);
        self.load()
    }

    fn back(&mut self) -> bool {
        match self.mode {
            Mode::Busy => false,
            Mode::Recording | Mode::Discard => {
                self.mode = Mode::Editing;
                self.status("", false);
                false
            }
            Mode::Editing if self.draft != self.original => {
                self.mode = Mode::Discard;
                self.status("Discard unsaved shortcuts?", false);
                false
            }
            Mode::Editing => true,
        }
    }

    fn command(&mut self, command: Command) -> Task<Event> {
        if self.mode != Mode::Editing {
            return Task::none();
        }
        match command {
            Command::Activate => self.start_edit(self.selected),
            Command::Up => self.select(self.selected.saturating_sub(1)),
            Command::Down => self.select(self.selected + 1),
            Command::First => self.select(0),
            Command::Last => self.select(RESET),
            _ => Task::none(),
        }
    }

    fn row(&self, slot: usize, label: &'static str, detail: Option<String>) -> Element<'static, Event> {
        let selected = slot == self.selected;
        let content = row![text(label).size(15), space::horizontal()].spacing(8).align_y(iced::Center);
        let content = if let Some(label) = detail { content.push(keycap(label, selected)) } else { content };
        button(content)
            .width(Length::Fill)
            .padding([6, 10])
            .on_press_maybe((self.mode == Mode::Editing && self.document.is_some()).then_some(Event::Select(slot)))
            .style(move |theme, status| tab_style(theme, status, selected, Some(palette::SELECTED)))
            .into()
    }

    fn view(&self) -> Element<'static, Event> {
        let back = button(text("Back").size(12)).on_press(Event::Back).padding([6, 8]).style(status_button_style);
        let reload = button(text("Reload").size(12))
            .on_press_maybe((self.mode == Mode::Editing).then_some(Event::Reload))
            .padding([6, 8])
            .style(status_button_style);
        let header = page_header(
            row![back, text("Shortcuts").size(20), space::horizontal(), reload].spacing(12).align_y(iced::Center),
        )
        .height(56);
        let choices = Action::ALL.into_iter().enumerate().fold(column![].spacing(4), |column, (slot, action)| {
            let binding = if self.recording() && slot == self.selected {
                "Recording…".into()
            } else {
                self.draft.get(action).to_string()
            };
            column.push(self.row(slot, action.label(), Some(binding)))
        });
        let notice = if self.notice.is_empty() { "Shortcuts apply while the launcher is open." } else { &self.notice };
        let color = if self.error { palette::ERROR } else { palette::DIM };
        let list = scrollable(choices.push(self.row(RESET, "Restore defaults", None)).padding(8)).id(SCROLL);
        let status = container(
            text(notice.to_owned())
                .size(13)
                .style(move |theme: &Theme| text::Style { color: Some(color.resolve(theme)) }),
        )
        .padding([4, 16]);
        // A keyed body gives the list its own state when replacing Settings.
        let body = keyed_column![(SCROLL, list.height(Length::Fill)), ("notice", status)].height(Length::Fill);
        container(column![header, divider(), body, divider(), self.footer()])
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into()
    }

    fn footer(&self) -> Element<'static, Event> {
        let back_label = match self.mode {
            Mode::Recording => "Cancel recording",
            Mode::Discard => "Keep editing",
            _ => "Back",
        };
        let back = button(hint("Esc", back_label)).on_press(Event::Back).padding([4, 6]).style(status_button_style);
        let mut actions = row![back, space::horizontal()].spacing(4).align_y(iced::Center);
        if self.mode == Mode::Discard {
            actions = actions
                .push(button(primary_hint("Ctrl+D", "Discard")).on_press(Event::Discard).style(status_button_style));
        } else if self.mode == Mode::Editing && self.draft != self.original {
            actions =
                actions.push(button(primary_hint("Ctrl+S", "Save")).on_press(Event::Save).style(status_button_style));
        } else if self.mode == Mode::Editing && self.document.is_some() {
            let label = if self.selected == RESET { "Restore defaults" } else { "Record" };
            actions = actions.push(
                button(primary_hint("Enter", label)).on_press(Event::Select(self.selected)).style(status_button_style),
            );
        }
        container(actions).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style).into()
    }
}

impl Launcher {
    pub(super) fn show_shortcuts(&mut self) -> Task<Message> {
        self.settings = self.settings.map(|_| super::settings::Entry::Shortcuts);
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.shortcuts.generation += 1;
        let panel = Panel::new(self.settings_path.clone(), self.shortcuts.bindings);
        let generation = self.shortcuts.generation;
        let load = panel.load().map(move |event| Message::ShortcutSettings { generation, event });
        self.shortcuts.panel = Some(panel);
        Task::batch([self.resize(), load])
    }

    pub(super) fn shortcut_settings_view(&self, panel: &Panel) -> Element<'static, Message> {
        let generation = self.shortcuts.generation;
        panel.view().map(move |event| Message::ShortcutSettings { generation, event })
    }

    pub(super) fn update_shortcut_settings(&mut self, message: Message) -> Task<Message> {
        let event = match message {
            Message::ShortcutSettings { generation, event } if generation == self.shortcuts.generation => event,
            Message::Cmd(Command::Back) => Event::Back,
            Message::FolderShortcut(folders::Shortcut::Save) => Event::Save,
            Message::FolderShortcut(folders::Shortcut::Remove) => Event::Discard,
            Message::RefreshSearchStatus => Event::Reload,
            Message::Cmd(command) => return self.shortcut_action(|panel| panel.command(command)),
            Message::ShortcutRecorded(chord) => return self.shortcut_action(|panel| panel.record(chord)),
            Message::ShowSettings | Message::ShowAppearance | Message::ShowFolders | Message::ShowSearchStatus => {
                return Task::none();
            }
            message => return update_content(self, message),
        };
        let Some(panel) = &mut self.shortcuts.panel else { return Task::none() };
        match event {
            Event::Back if panel.back() => return self.close_shortcuts(),
            Event::Discard if panel.mode == Mode::Discard => return self.close_shortcuts(),
            Event::Loaded(result) => panel.loaded(result),
            Event::Saved(result) => panel.saved(result),
            Event::Select(slot) => return self.shortcut_action(|panel| panel.start_edit(slot)),
            Event::Save => return self.shortcut_action(Panel::save),
            Event::Reload => return self.shortcut_action(Panel::reload),
            _ => {}
        }
        self.shortcuts.bindings = panel.original;
        Task::none()
    }

    fn shortcut_action(&mut self, action: impl FnOnce(&mut Panel) -> Task<Event>) -> Task<Message> {
        let generation = self.shortcuts.generation;
        self.shortcuts
            .panel
            .as_mut()
            .map(action)
            .unwrap_or_else(Task::none)
            .map(move |event| Message::ShortcutSettings { generation, event })
    }

    fn close_shortcuts(&mut self) -> Task<Message> {
        self.shortcuts.panel = None;
        self.return_to_parent()
    }
}
