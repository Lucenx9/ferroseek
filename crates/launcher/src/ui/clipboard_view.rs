// SPDX-License-Identifier: GPL-3.0-or-later
//! Search and manage saved clipboard text.

use iced::futures::{StreamExt, stream};
use iced::widget::operation::{self, AbsoluteOffset, RelativeOffset};
use iced::widget::{Id, button, column, container, mouse_area, responsive, row, scrollable, space, text, text_input};
use iced::{Element, Length, Task, Theme};

use super::{
    Launcher, Message, divider, field_style, footer_style, hint, keycap, label_for_width, page_header, palette,
    primary_hint, status_button_style, surface_style, tab_style, update_content,
};
use crate::clipboard_history::publish;
use crate::clipboard_history::{Edit, Entry, Mode, Profile, Recorder, Snapshot};
use crate::layout::{FOOTER_HEIGHT, PAD, ROW_HEIGHT};
use crate::model::{Command, Cursor, NUMBERED_SLOTS};
use crate::{folders, timing, usage};

const HEIGHT: u32 = 536;
const EMPTY_HEIGHT: u32 = 292;
const LIST_HEADER: u32 = 198;
const PASTE_BAR_HEIGHT: u32 = 30;

#[derive(Debug, Clone, Copy)]
pub enum Shortcut {
    Clear,
    Retention,
    Pin,
    PinnedOnly,
    Paste,
}

#[derive(Debug, Clone)]
pub enum Event {
    Query(String),
    Select(u64),
    DoubleClick(u64),
    Preview(u64),
    ScrollPreview(Command),
    Load(Result<Snapshot, String>),
    Edited(Result<Snapshot, String>),
    Toggle,
    Retention,
    Clear,
    Confirm,
    Delete(u64),
    Pin(u64),
    PinnedOnly,
    Refresh,
    Copy(u64),
    Copied(Option<String>),
    Paste(u64),
    Back,
}

pub(super) struct State {
    private: bool,
    generation: u64,
    pub(super) panel: Option<Panel>,
}

impl State {
    pub(super) fn new(private: bool) -> Self {
        Self { private, generation: 0, panel: None }
    }
}

#[derive(Debug)]
enum Pending {
    Edit,
    Copy,
    Paste,
}

pub(super) struct Panel {
    profile: Option<Profile>,
    private: bool,
    snapshot: Option<Snapshot>,
    query: String,
    pinned_only: bool,
    matches: Vec<usize>,
    cursor: Cursor,
    press: Option<(u64, bool)>,
    input: Id,
    confirm_context: Id,
    preview_scroll: Id,
    preview: Option<u64>,
    feed: Option<iced::task::Handle>,
    pending: Option<Pending>,
    confirm: bool,
    error: Option<String>,
    note: &'static str,
}

impl Panel {
    fn new(profile: Option<Profile>, private: bool) -> Self {
        Self {
            profile,
            private,
            snapshot: None,
            query: String::new(),
            pinned_only: false,
            matches: Vec::new(),
            cursor: Cursor::new(NUMBERED_SLOTS),
            press: None,
            input: Id::unique(),
            confirm_context: Id::unique(),
            preview_scroll: Id::unique(),
            preview: None,
            feed: None,
            pending: None,
            confirm: false,
            error: None,
            note: "",
        }
    }

    pub(super) fn focus(&self) -> Option<Id> {
        (self.profile.is_some() && self.preview.is_none() && !self.confirm).then(|| self.input.clone())
    }

    pub(super) fn context(&self) -> Id {
        if self.confirm {
            self.confirm_context.clone()
        } else if self.preview.is_some() {
            self.preview_scroll.clone()
        } else {
            self.input.clone()
        }
    }

    pub(super) fn height(&self) -> u32 {
        if self.entries().is_empty() && self.preview.is_none() { EMPTY_HEIGHT } else { HEIGHT }
    }

    fn viewed(&self) -> Option<&Entry> {
        let id = self.preview?;
        self.entries().iter().find(|entry| entry.id == id)
    }

    fn entries(&self) -> &[Entry] {
        self.snapshot.as_ref().map_or(&[], |snapshot| &snapshot.entries)
    }

    fn selected(&self) -> Option<&Entry> {
        self.matches.get(self.cursor.selected).and_then(|&index| self.entries().get(index))
    }

    fn copy_target(&self) -> Option<&Entry> {
        if self.preview.is_some() { self.viewed() } else { self.selected() }
    }

    fn copy_text(&self, id: u64) -> Option<String> {
        self.copy_target().filter(|entry| entry.id == id && self.can_act()).map(|entry| entry.text.clone())
    }

    pub(super) fn paste_feedback(&mut self, error: Option<&str>) {
        self.pending = error.is_none().then_some(Pending::Paste);
        self.error = error.map(str::to_owned);
        self.note = "";
    }

    fn filter(&mut self, selected: Option<u64>) {
        let terms = crate::matcher::tokens(&self.query);
        self.matches = self
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                ((!self.pinned_only || entry.pinned) && matches_text(&terms, &entry.text)).then_some(index)
            })
            .collect();
        let selected = selected.and_then(|id| self.matches.iter().position(|&index| self.entries()[index].id == id));
        self.cursor.reset();
        if let Some(index) = selected {
            self.cursor.select(index, self.matches.len());
        }
    }

    fn received(&mut self, result: Result<Snapshot, String>) {
        match result {
            Ok(snapshot) => {
                let selected = self.selected().map(|entry| entry.id);
                self.snapshot = Some(snapshot);
                self.filter(selected);
                timing::event("clipboard_history_ready");
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn update_rows(&mut self, height: u32) {
        self.cursor.visible =
            ((height.saturating_sub(LIST_HEADER) / ROW_HEIGHT as u32) as usize).clamp(1, NUMBERED_SLOTS);
        self.cursor.move_by(0, self.matches.len());
    }

    fn start_feed(&mut self) -> Task<Event> {
        let Some(profile) = self.profile.clone() else { return Task::none() };
        let events = stream::unfold((profile, None), |(profile, previous)| async move {
            let (profile, stamp, update) = zruntime::unblock(move || {
                if previous.is_some() {
                    std::thread::sleep(std::time::Duration::from_millis(450));
                }
                let stamp = profile.stamp();
                let update = (previous != Some(stamp)).then(|| profile.snapshot().map_err(|error| error.to_string()));
                (profile, stamp, update)
            })
            .await;
            Some((update, (profile, Some(stamp))))
        })
        .filter_map(iced::futures::future::ready);
        let (task, handle) = Task::run(events, Event::Load).abortable();
        self.feed = Some(handle.abort_on_drop());
        task
    }

    fn edit(&mut self, edit: Option<Edit>) -> Task<Event> {
        if self.pending.is_some() {
            return Task::none();
        }
        let Some(profile) = self.profile.clone() else { return Task::none() };
        self.confirm = false;
        self.pending = Some(Pending::Edit);
        self.error = None;
        self.note = "";
        let update = async move {
            zruntime::unblock(move || {
                if let Some(edit) = edit {
                    profile.edit(edit)?;
                }
                profile.ensure_running()?;
                profile.snapshot()
            })
            .await
            .map_err(|error| error.to_string())
        };
        Task::perform(update, Event::Edited)
    }

    fn toggle(&mut self) -> Task<Event> {
        let recording = self.snapshot.as_ref().is_some_and(|snapshot| snapshot.policy.mode == Mode::Recording);
        let edit = if recording { Edit::Stop } else { Edit::Enable };
        self.edit(Some(edit))
    }

    fn retention(&mut self) -> Task<Event> {
        let Some(snapshot) = &self.snapshot else { return Task::none() };
        self.edit(Some(Edit::Retain(snapshot.policy.retention.next())))
    }

    fn command(&mut self, command: Command) {
        let length = self.matches.len();
        let page = self.cursor.visible as isize;
        let movement = match command {
            Command::Up => -1,
            Command::Down => 1,
            Command::PageUp => -page,
            Command::PageDown => page,
            Command::First => -(length as isize),
            Command::Last => length as isize,
            _ => return,
        };
        self.cursor.move_by(movement, length);
    }

    fn select(&mut self, id: u64) {
        let repeated = self.press.is_some_and(|(last, _)| last == id);
        self.press = Some((id, repeated));
        if let Some(index) = self.matches.iter().position(|&index| self.entries()[index].id == id) {
            self.cursor.select(index, self.matches.len());
        }
    }

    fn can_act(&self) -> bool {
        !self.confirm && self.pending.is_none()
    }

    fn key_command(&mut self, command: Command) -> Option<Event> {
        if command == Command::Back {
            return Some(Event::Back);
        }
        if !self.can_act() {
            return None;
        }
        self.press = None;
        if self.preview.is_some() {
            return self.preview_key(command);
        }
        match command {
            Command::Activate => self.selected().map(|entry| Event::Copy(entry.id)),
            Command::Secondary => self.selected().map(|entry| Event::Preview(entry.id)),
            Command::ActivateVisible(slot) if self.cursor.select_visible(slot, self.matches.len()) => {
                self.selected().map(|entry| Event::Copy(entry.id))
            }
            _ => {
                self.command(command);
                None
            }
        }
    }

    fn preview_key(&self, command: Command) -> Option<Event> {
        match command {
            Command::Secondary => Some(Event::Back),
            Command::Activate => self.viewed().map(|entry| Event::Copy(entry.id)),
            Command::Up | Command::Down | Command::PageUp | Command::PageDown | Command::First | Command::Last => {
                Some(Event::ScrollPreview(command))
            }
            _ => None,
        }
    }

    fn remove_event(&self) -> Option<Event> {
        if self.confirm { Some(Event::Confirm) } else { self.selected().map(|entry| Event::Delete(entry.id)) }
    }

    fn input_event(&mut self, message: Message, generation: u64) -> Option<Event> {
        match message {
            Message::Clipboard { generation: current, event } if current == generation => Some(event),
            Message::ClipboardShortcut(shortcut) => shortcut.event(self),
            Message::Cmd(command) => self.key_command(command),
            Message::FolderShortcut(folders::Shortcut::Remove) => self.remove_event(),
            Message::FolderShortcut(folders::Shortcut::Save) => Some(Event::Toggle),
            Message::RefreshSearchStatus => Some(Event::Refresh),
            Message::Wheel(delta) if self.preview.is_none() && self.can_act() => {
                self.cursor.move_by(-(delta.signum() as isize), self.matches.len());
                None
            }
            _ => None,
        }
    }

    fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Query(query) if self.can_act() => {
                self.query = query;
                self.note = "";
                self.filter(None);
            }
            Event::Select(id) if self.can_act() => self.select(id),
            event => return self.control_event(event),
        }
        Task::none()
    }

    fn control_event(&mut self, event: Event) -> Task<Event> {
        if self.confirm {
            return if matches!(event, Event::Confirm) { self.edit(Some(Edit::Clear)) } else { Task::none() };
        }
        if self.preview.is_some() {
            return Task::none();
        }
        match event {
            Event::Toggle => self.toggle(),
            Event::Retention => self.retention(),
            Event::Refresh => self.edit(None),
            Event::Clear => {
                self.confirm = !self.entries().is_empty() && self.pending.is_none();
                Task::none()
            }
            Event::Delete(id) => self.edit(Some(Edit::Delete(id))),
            Event::Pin(id) => {
                let edit =
                    self.selected().filter(|entry| entry.id == id).map(|entry| Edit::Pin { id, pinned: !entry.pinned });
                edit.map_or_else(Task::none, |edit| self.edit(Some(edit)))
            }
            Event::PinnedOnly if self.can_act() && self.snapshot.is_some() => {
                let selected = self.selected().map(|entry| entry.id);
                self.pinned_only = !self.pinned_only;
                self.note = "";
                self.filter(selected);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn double_clicked(&mut self, id: u64) -> bool {
        self.press.take() == Some((id, true)) && self.can_act()
    }

    fn view(&self, generation: u64, width: u32, paste_label: Option<&str>) -> Element<'_, Message> {
        let label = if width < 400 { "Clipboard" } else { "Clipboard history" };
        let mut title = row![text(label).size(20), space::horizontal()].spacing(8).align_y(iced::Center);
        if self.preview.is_none() {
            if self.snapshot.is_some() {
                title = title.push(self.pin_filter(width));
            }
            title = title.push(
                button(text("Refresh").size(12))
                    .on_press_maybe((self.profile.is_some() && self.can_act()).then_some(Event::Refresh))
                    .padding([6, 8])
                    .style(status_button_style),
            );
        }
        let title = page_header(title).height(56);
        let content = if self.preview.is_some() {
            self.preview_view()
        } else {
            column![self.search(), self.controls(width), self.rows()].height(Length::Fill).into()
        };
        let mut body = column![title, divider(), content];
        if let Some(label) = paste_label {
            body = body.push(self.paste_bar(label, width));
        }
        let body = body.push(divider()).push(self.footer(width));
        let view = container(body).padding(PAD).width(Length::Fill).height(Length::Fill).style(surface_style);
        crate::shortcuts::capture_numbered(
            Element::from(view).map(move |event| Message::Clipboard { generation, event }),
        )
    }

    fn paste_bar(&self, label: &str, width: u32) -> Element<'_, Event> {
        let destination = format!("Paste into {label}");
        let destination = responsive(move |size| {
            text(label_for_width(&destination, size.width, 12.0, false)).size(12).wrapping(text::Wrapping::None).into()
        })
        .width(Length::Fill)
        .height(Length::Shrink);
        let action = if width < 460 { text("Paste").size(12).into() } else { hint("Alt+Enter", "Paste") };
        let target = self.copy_target().filter(|_| self.can_act()).map(|entry| Event::Paste(entry.id));
        container(
            row![destination, button(action).on_press_maybe(target).padding([4, 8]).style(status_button_style)]
                .spacing(8)
                .align_y(iced::Center),
        )
        .padding([2, 10])
        .height(PASTE_BAR_HEIGHT)
        .align_y(iced::Center)
        .into()
    }

    fn pin_filter(&self, width: u32) -> Element<'_, Event> {
        let active = self.pinned_only;
        let label = if active { "Show all" } else { "Pinned" };
        let content = if width < 460 { text(label).size(12).into() } else { hint("Ctrl+G", label) };
        button(content)
            .on_press_maybe(self.can_act().then_some(Event::PinnedOnly))
            .padding([6, 8])
            .style(move |theme, status| tab_style(theme, status, active, Some(palette::SELECTED)))
            .into()
    }

    fn preview_view(&self) -> Element<'_, Event> {
        let value = self.viewed().map_or("This entry was removed.", |entry| entry.text.as_str());
        let preview = scrollable(
            container(text(value).font(iced::Font::MONOSPACE).size(14).wrapping(text::Wrapping::WordOrGlyph))
                .padding(14)
                .width(Length::Fill),
        )
        .id(self.preview_scroll.clone())
        .height(Length::Fill);
        if self.error.is_some() || self.pending.is_some() {
            column![self.notice(), preview].height(Length::Fill).into()
        } else {
            preview.into()
        }
    }

    fn begin_preview(&mut self, id: u64) -> Task<Message> {
        if self.selected().is_none_or(|entry| entry.id != id) {
            return Task::none();
        }
        self.preview = Some(id);
        timing::event("clipboard_preview_opened");
        operation::snap_to(self.preview_scroll.clone(), RelativeOffset::START)
    }

    fn scroll_preview(&self, command: Command) -> Task<Message> {
        let scroll = self.preview_scroll.clone();
        let y = match command {
            Command::Up => -36.0,
            Command::Down => 36.0,
            Command::PageUp => -216.0,
            Command::PageDown => 216.0,
            Command::First => return operation::snap_to(scroll, RelativeOffset::START),
            Command::Last => return operation::snap_to(scroll, RelativeOffset::END),
            _ => return Task::none(),
        };
        operation::scroll_by(scroll, AbsoluteOffset { x: 0.0, y })
    }

    fn search(&self) -> Element<'_, Event> {
        let mut search =
            text_input("Search copied text…", &self.query).id(self.input.clone()).padding([6, 8]).style(field_style);
        if self.profile.is_some() && !self.confirm {
            search = search.on_input(Event::Query);
        }
        container(search).padding([4, 8]).into()
    }

    fn controls(&self, width: u32) -> Element<'_, Event> {
        let Some(snapshot) = &self.snapshot else {
            return space().height(4).into();
        };
        let toggle = match snapshot.policy.mode {
            Mode::Recording => "Stop recording",
            Mode::Paused => "Resume recording",
            Mode::Off => "Start recording",
        };
        let enabled = self.can_act();
        let control = |label, event, enabled: bool| {
            button(text(label).size(12)).on_press_maybe(enabled.then_some(event)).padding(6).style(status_button_style)
        };
        let days = format!("Keep {} days", snapshot.policy.retention.days());
        let clear = if width < 460 { "Clear" } else { "Clear history" };
        container(
            row![
                control(toggle.to_owned(), Event::Toggle, enabled),
                control(days, Event::Retention, enabled),
                space::horizontal(),
                control(clear.into(), Event::Clear, enabled && !snapshot.entries.is_empty()),
            ]
            .spacing(4)
            .align_y(iced::Center),
        )
        .padding([0, 8])
        .into()
    }

    fn message(&self) -> (&str, palette::Role) {
        if self.confirm {
            return ("Clear all entries, including pins?", palette::TEXT);
        }
        if let Some(error) = &self.error {
            return (error, palette::ERROR);
        }
        if let Some(operation) = &self.pending {
            let message = match operation {
                Pending::Edit => "Saving clipboard history…",
                Pending::Copy => "Copying text…",
                Pending::Paste => "Pasting text…",
            };
            return (message, palette::DIM);
        }
        if self.private {
            return ("Clipboard history is paused for private use.", palette::DIM);
        }
        if self.profile.is_none() {
            return ("Clipboard history needs a local state folder.", palette::DIM);
        }
        let message = self.snapshot.as_ref().map_or("Loading clipboard history…", recorder_message);
        (if self.note.is_empty() { message } else { self.note }, palette::DIM)
    }

    fn notice(&self) -> Element<'_, Event> {
        let (message, role) = self.message();
        container(text(message).size(12).style(move |theme: &Theme| text::Style { color: Some(role.resolve(theme)) }))
            .padding([4, 10])
            .into()
    }

    fn rows(&self) -> Element<'_, Event> {
        let empty = self.snapshot.is_some() && self.matches.is_empty() && !self.confirm;
        let list = if empty {
            let message = match (self.pinned_only, self.query.is_empty()) {
                (true, true) => "No pinned text. Show all to pin an entry.",
                (true, false) => "No matching pins. Show all or try another search.",
                (false, true) => "No copied text yet.",
                (false, false) => "No matching text. Try another search.",
            };
            column![container(text(message).size(13)).padding([6, 10])]
        } else {
            self.cursor.window(self.matches.len()).enumerate().fold(column![], |column, (slot, index)| {
                let entry = &self.entries()[self.matches[index]];
                column.push(history_row(entry, slot, index == self.cursor.selected, self.can_act()))
            })
        };
        column![self.notice(), list].height(Length::Fill).padding([0, 4]).into()
    }

    fn footer(&self, width: u32) -> Element<'_, Event> {
        let label = match (self.confirm, self.preview.is_some()) {
            (true, _) => "Keep history",
            (_, true) => "Back to history",
            _ => "Back",
        };
        let back_label = if width < 360 { text(label).size(12).into() } else { hint("Esc", label) };
        let back = button(back_label).on_press(Event::Back).padding([4, 6]).style(status_button_style);
        let mut actions = row![back, space::horizontal()].spacing(4).align_y(iced::Center);
        if self.confirm {
            actions = actions
                .push(button(primary_hint("Ctrl+D", "Clear")).on_press(Event::Confirm).style(status_button_style));
        } else if let Some(entry) = self.viewed() {
            actions = actions.push(text(self.note).size(12)).push(copy_button(entry.id, true));
        } else if self.preview.is_none() {
            for action in self.list_actions(width) {
                actions = actions.push(action);
            }
        }
        container(actions).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style).into()
    }

    fn list_actions(&self, width: u32) -> Vec<Element<'_, Event>> {
        let Some(entry) = self.selected().filter(|_| self.can_act()) else { return Vec::new() };
        let action = |key, label, event| {
            let content = if width < 600 { text(label).size(12).into() } else { hint(key, label) };
            button(content).on_press(event).style(status_button_style).into()
        };
        vec![
            action("Ctrl+Enter", "Preview", Event::Preview(entry.id)),
            action("Ctrl+Shift+P", if entry.pinned { "Unpin" } else { "Pin" }, Event::Pin(entry.id)),
            action("Ctrl+D", "Delete", Event::Delete(entry.id)),
            copy_button(entry.id, width >= 360),
        ]
    }
}

fn copy_button(id: u64, shortcut: bool) -> Element<'static, Event> {
    let label = if shortcut {
        primary_hint("Enter", "Copy")
    } else {
        text("Copy").size(12).font(iced::Font { weight: iced::font::Weight::Semibold, ..iced::Font::DEFAULT }).into()
    };
    button(label).on_press(Event::Copy(id)).style(status_button_style).into()
}

impl Shortcut {
    fn event(self, panel: &Panel) -> Option<Event> {
        match self {
            Self::Clear => Some(Event::Clear),
            Self::Retention => Some(Event::Retention),
            Self::Pin => panel.selected().map(|entry| Event::Pin(entry.id)),
            Self::PinnedOnly => Some(Event::PinnedOnly),
            Self::Paste => panel.copy_target().map(|entry| Event::Paste(entry.id)),
        }
    }
}

fn recorder_message(snapshot: &Snapshot) -> &'static str {
    match (snapshot.policy.mode, snapshot.recorder) {
        (Mode::Recording, Recorder::Ready) => "Recording copied text",
        (Mode::Recording, Recorder::Starting) => "Starting clipboard recorder…",
        (Mode::Recording, Recorder::Idle) => "Recorder unavailable. Use Refresh to try again.",
        (Mode::Paused, _) => "Recording paused. Resume to capture new copies.",
        (Mode::Off, _) => "Start recording to save new copies, including while the launcher is closed.",
    }
}

fn matches_text(terms: &[String], value: &str) -> bool {
    terms.is_empty() || {
        let value = crate::matcher::fold(value);
        terms.iter().all(|term| value.contains(term))
    }
}

fn history_row(entry: &Entry, slot: usize, selected: bool, enabled: bool) -> Element<'static, Event> {
    let mut lines = entry.text.lines().map(str::trim).filter(|line| !line.is_empty());
    let title: String = lines.next().unwrap_or("Copied text").chars().take(255).collect();
    let age = usage::now().saturating_sub(entry.copied_at);
    let when = match age {
        0..60 => "Just now".into(),
        60..3600 => format!("{} min ago", age / 60),
        3600..86400 => format!("{} hr ago", age / 3600),
        _ => format!("{} days ago", age / 86400),
    };
    let subtitle = if entry.pinned { format!("Pinned · {when}") } else { when };
    let title = responsive(move |size| {
        text(label_for_width(&title, size.width, 15.0, false)).size(15).wrapping(text::Wrapping::None).into()
    })
    .height(Length::Shrink);
    let item = row![
        column![title, text(subtitle).size(12)].width(Length::Fill).spacing(2),
        keycap(format!("Ctrl+{}", slot + 1), selected),
    ]
    .spacing(8)
    .align_y(iced::Center);
    let row = super::selectable_result_row(item.padding([0, 10]), selected);
    if enabled {
        mouse_area(row)
            .on_press(Event::Select(entry.id))
            .on_double_click(Event::DoubleClick(entry.id))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    } else {
        row
    }
}

impl Launcher {
    pub(super) fn show_clipboard_history(&mut self) -> Task<Message> {
        self.panel = None;
        self.clipboard_history.generation += 1;
        let private = self.clipboard_history.private;
        let profile = if private { None } else { Profile::from_env() };
        let mut panel = Panel::new(profile, private);
        let generation = self.clipboard_history.generation;
        let feed = panel.start_feed().map(move |event| Message::Clipboard { generation, event });
        self.clipboard_history.panel = Some(panel);
        Task::batch([self.resize(), feed])
    }

    pub(super) fn clipboard_view<'a>(&self, panel: &'a Panel) -> Element<'a, Message> {
        panel.view(self.clipboard_history.generation, self.window_width, self.paste.label())
    }

    pub(super) fn resize_clipboard(&mut self, height: u32) {
        if let Some(panel) = &mut self.clipboard_history.panel {
            let bar = if self.paste.label().is_some() { PASTE_BAR_HEIGHT } else { 0 };
            panel.update_rows(height.saturating_sub(bar));
        }
    }

    pub(super) fn update_clipboard_history(&mut self, message: Message) -> Task<Message> {
        let generation = self.clipboard_history.generation;
        let panel = self.clipboard_history.panel.as_mut().expect("clipboard panel is open");
        if matches!((&message, &panel.pending), (Message::FocusLost, Some(Pending::Copy))) {
            return Task::none();
        }
        if matches!(&message, Message::Files { .. } | Message::Ui(_) | Message::FocusLost | Message::InputClipboard(_))
        {
            return update_content(self, message);
        }
        let Some(event) = panel.input_event(message, generation) else { return Task::none() };
        self.clipboard_event(event)
    }

    fn clipboard_event(&mut self, event: Event) -> Task<Message> {
        let generation = self.clipboard_history.generation;
        let panel = self.clipboard_history.panel.as_mut().expect("clipboard panel is open");
        match event {
            Event::Load(result) if !matches!(panel.pending, Some(Pending::Edit)) => {
                self.clipboard_loaded(result, false)
            }
            Event::Edited(result) => self.clipboard_loaded(result, true),
            Event::Copied(result) => self.history_copy_finished(result),
            Event::Copy(id) | Event::DoubleClick(id) => {
                self.copy_history_entry(id, matches!(event, Event::DoubleClick(_)))
            }
            Event::Paste(id) => {
                let text = panel.copy_text(id);
                text.map_or_else(Task::none, |text| self.paste_text(text, super::paste::Origin::History))
            }
            Event::Preview(id) if panel.can_act() => panel.begin_preview(id),
            Event::ScrollPreview(command) => panel.scroll_preview(command),
            Event::Back => self.back_from_clipboard(),
            event => panel.update(event).map(move |event| Message::Clipboard { generation, event }),
        }
    }

    fn clipboard_loaded(&mut self, result: Result<Snapshot, String>, edited: bool) -> Task<Message> {
        let panel = self.clipboard_history.panel.as_mut().unwrap();
        let height = panel.height();
        if edited {
            panel.pending = None;
        }
        panel.received(result);
        let focus = if edited { panel.focus().map_or_else(Task::none, crate::input::focus) } else { Task::none() };
        if height == panel.height() { focus } else { Task::batch([focus, self.resize()]) }
    }

    fn copy_history_entry(&mut self, id: u64, double_click: bool) -> Task<Message> {
        let panel = self.clipboard_history.panel.as_mut().unwrap();
        if double_click && !panel.double_clicked(id) {
            return Task::none();
        }
        let Some(text) = panel.copy_text(id) else { return Task::none() };
        panel.pending = Some(Pending::Copy);
        panel.error = None;
        panel.note = "Copying text…";
        let generation = self.clipboard_history.generation;
        let handoff = zruntime::unblock(move || publish::handoff(&text).err().map(|_| text));
        Task::perform(handoff, move |result| Message::Clipboard { generation, event: Event::Copied(result) })
    }

    fn history_copy_finished(&mut self, fallback: Option<String>) -> Task<Message> {
        let panel = self.clipboard_history.panel.as_mut().unwrap();
        panel.pending = None;
        panel.note = "Copied text";
        let focus = panel.focus().map_or_else(Task::none, crate::input::focus);
        match fallback {
            Some(text) => Task::batch([self.write_clipboard(text), focus]),
            None => {
                timing::event("clipboard_handoff_ready");
                self.clipboard_owner = false;
                focus
            }
        }
    }

    fn back_from_clipboard(&mut self) -> Task<Message> {
        let panel = self.clipboard_history.panel.as_mut().unwrap();
        if panel.pending.is_some() {
            return Task::none();
        }
        if panel.preview.take().is_some() {
            timing::event("clipboard_preview_closed");
            return Task::batch([panel.focus().map_or_else(Task::none, crate::input::focus), self.resize()]);
        }
        if panel.confirm {
            panel.confirm = false;
            return panel.focus().map_or_else(Task::none, crate::input::focus);
        }
        if !panel.query.is_empty() {
            panel.query.clear();
            panel.filter(None);
            return panel.focus().map_or_else(Task::none, crate::input::focus);
        }
        self.clipboard_history.panel = None;
        self.return_to_parent()
    }
}
