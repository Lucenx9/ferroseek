// SPDX-License-Identifier: GPL-3.0-or-later
//! Search open windows without changing the parent search.

use std::time::Duration;

use iced::widget::{Id, button, column, container, image, mouse_area, responsive, row, space, svg, text, text_input};
use iced::{Element, Length, Task, Theme};
use iced_layershell::reexport::KeyboardInteractivity;

use super::{
    ICON_SIZE, Launcher, Message, divider, field_style, footer_button, footer_style, hint, keyed_footer_button,
    label_for_width, numbered_slot, page_header, palette, primary_hint, search_id, selectable_result_row,
    status_button_style, surface_style, update_content,
};
use crate::apps::App;
use crate::icons::{Icon, Icons};
use crate::layout::{FOOTER_HEIGHT, PAD, ROW_HEIGHT};
use crate::model::{Command, Cursor, NUMBERED_SLOTS};
use crate::window_switcher::{Action, Entry, Update, client::Client};

const LIST_CHROME: u32 = 172;
pub(super) const HEIGHT: u32 = LIST_CHROME + ROW_HEIGHT as u32 * 5;
const KEYBOARD_TIMEOUT: Duration = Duration::from_millis(750);
const RELEASE_FAILED: &str = "Could not release keyboard focus. Try switching again.";

#[derive(Debug, Clone)]
pub enum Event {
    Update(Update),
    Query(String),
    Press(u64),
    DoubleClick(u64),
    OpenActions(u64),
    Run(u64, Action),
    Apply(u64, Action),
    ReleaseTimedOut(u64),
    RestoreTimedOut(u64),
    Finished(Result<(), String>),
    Back,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) generation: u64,
    pub(super) panel: Option<Panel>,
}

impl State {
    pub(super) fn resize(&mut self, height: u32) {
        if let Some(panel) = &mut self.panel {
            panel.cursor.visible =
                ((height.saturating_sub(LIST_CHROME) / ROW_HEIGHT as u32) as usize).clamp(1, NUMBERED_SLOTS);
            panel.cursor.move_by(0, panel.matches.len());
        }
    }
}

pub(super) struct Panel {
    input: Id,
    query: String,
    windows: Option<Vec<WindowRow>>,
    matches: Vec<usize>,
    cursor: Cursor,
    selected_closed: bool,
    press: Option<(u64, bool)>,
    session: Option<Client>,
    pending: Option<Pending>,
    switch_attempt: u64,
    menu: Option<usize>,
    confirmation: Option<&'static str>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Releasing(u64),
    Applying(Action),
    Restoring,
}

impl Pending {
    fn switching(self) -> bool {
        matches!(self, Self::Releasing(_) | Self::Applying(Action::Switch))
    }
}

impl Panel {
    fn new() -> Self {
        Self {
            input: Id::unique(),
            query: String::new(),
            windows: None,
            matches: Vec::new(),
            cursor: Cursor::new(NUMBERED_SLOTS),
            selected_closed: false,
            press: None,
            session: None,
            pending: None,
            switch_attempt: 0,
            menu: None,
            confirmation: None,
            error: None,
        }
    }

    pub(super) fn input(&self) -> Id {
        self.input.clone()
    }

    fn windows(&self) -> &[WindowRow] {
        self.windows.as_deref().unwrap_or_default()
    }

    fn selected(&self) -> Option<u64> {
        self.selected_entry().map(|entry| entry.id)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        if self.selected_closed {
            return None;
        }
        self.matches.get(self.cursor.selected).map(|&index| &self.windows()[index].entry)
    }

    fn refresh_matches(&mut self, selection: Option<u64>) {
        let terms = crate::matcher::tokens(&self.query);
        self.matches =
            self.windows().iter().enumerate().filter_map(|(i, window)| window.matches(&terms).then_some(i)).collect();
        self.cursor.move_by(0, self.matches.len());
        self.selected_closed = selection.is_some_and(|id| !self.select(id));
    }

    fn select(&mut self, id: u64) -> bool {
        let Some(index) = self.matches.iter().position(|&index| self.windows()[index].entry.id == id) else {
            return false;
        };
        self.cursor.select(index, self.matches.len());
        self.selected_closed = false;
        true
    }

    fn receive(&mut self, update: Update, apps: &[App], icons: &mut Icons) -> Task<Message> {
        match update {
            Update::Windows(windows) => {
                let selection = self.selected();
                self.windows = Some(windows.into_iter().map(|entry| WindowRow::new(entry, apps, icons)).collect());
                let closed = self.selected_closed;
                self.refresh_matches(selection);
                self.selected_closed |= closed;
            }
            Update::Unavailable(message) => {
                self.menu = None;
                self.session = None;
                self.error = Some(message);
            }
        }
        if self.selected().is_none() && self.menu.take().is_some() {
            return crate::input::focus(self.input());
        }
        Task::none()
    }

    fn change_query(&mut self, query: String) {
        self.query = query;
        self.cursor.reset();
        self.press = None;
        self.error = None;
        self.confirmation = None;
        self.refresh_matches(None);
    }

    fn press(&mut self, id: u64) {
        if self.select(id) {
            self.press = Some((id, self.press.is_some_and(|(previous, _)| previous == id)));
            self.confirmation = None;
        }
    }

    fn command(&mut self, command: Command) -> Option<Event> {
        if let Some(choice) = self.menu {
            return self.menu_command(command, choice);
        }
        let page = self.cursor.visible as isize;
        let delta = match command {
            Command::Up => -1,
            Command::Down => 1,
            Command::PageUp => -page,
            Command::PageDown => page,
            Command::First => -(self.cursor.selected as isize),
            Command::Last => self.matches.len() as isize,
            Command::Activate => return self.selected().map(|id| Event::Run(id, Action::Switch)),
            Command::OpenActions | Command::ToggleActions => return self.selected().map(Event::OpenActions),
            Command::ActivateVisible(slot) => {
                if self.cursor.select_visible(slot, self.matches.len()) {
                    self.selected_closed = false;
                    return self.selected().map(|id| Event::Run(id, Action::Switch));
                }
                return None;
            }
            Command::Back => return Some(Event::Back),
            _ => return None,
        };
        self.move_by(delta);
        None
    }

    fn menu_command(&mut self, command: Command, selected: usize) -> Option<Event> {
        let choice = match command {
            Command::Back | Command::ToggleActions => return Some(Event::Back),
            Command::Up | Command::First | Command::PageUp => 0,
            Command::Down | Command::Last | Command::PageDown => 1,
            Command::Activate => return self.menu_action(selected),
            Command::ActivateVisible(index) => return self.menu_action(index),
            _ => return None,
        };
        self.menu = Some(choice);
        None
    }

    fn menu_action(&self, index: usize) -> Option<Event> {
        let entry = self.selected_entry()?;
        window_actions(entry).get(index).map(|&action| Event::Run(entry.id, action))
    }

    fn move_by(&mut self, delta: isize) {
        self.cursor.move_by(delta, self.matches.len());
        self.selected_closed = false;
        self.press = None;
        self.confirmation = None;
    }

    fn retains_focus(&self) -> bool {
        self.pending.is_some_and(|pending| pending.switching() || pending == Pending::Restoring)
    }

    fn keyboard_restored(&mut self) {
        if self.pending == Some(Pending::Restoring) {
            self.pending = None;
        }
    }

    fn interactive(&self) -> bool {
        self.session.is_some() && self.pending.is_none()
    }

    fn run(&mut self, id: u64, action: Action, generation: u64) -> Task<Message> {
        if self.selected() != Some(id) || self.menu.is_some() == (action == Action::Switch) {
            return Task::none();
        }
        self.error = None;
        self.confirmation = None;
        if action != Action::Switch {
            return self.request(id, action, generation);
        }

        self.pending = Some(Pending::Releasing(id));
        self.switch_attempt += 1;
        let attempt = self.switch_attempt;
        // The helper uses a separate Wayland connection. Wait for keyboard
        // leave before sending it an activation request.
        Task::done(Message::KeyboardInteractivityChange(KeyboardInteractivity::None))
            .chain(keyboard_timeout(generation, Event::ReleaseTimedOut(attempt)))
    }

    fn keyboard_released(&mut self, generation: u64) -> Task<Message> {
        let Some(Pending::Releasing(id)) = self.pending else { return Task::none() };
        self.request(id, Action::Switch, generation)
    }

    fn request(&mut self, id: u64, action: Action, generation: u64) -> Task<Message> {
        let Some(session) = &mut self.session else {
            let error =
                self.error.clone().unwrap_or_else(|| "Could not read open windows. Reopen Switch windows.".into());
            return self.finish(Err(error), generation);
        };
        self.pending = Some(Pending::Applying(action));
        Task::perform(session.perform(id, action), move |result| Message::Windows {
            generation,
            event: Event::Finished(result),
        })
    }

    fn event(&mut self, event: Event, generation: u64) -> Task<Message> {
        match event {
            Event::Finished(result) if self.pending != Some(Pending::Restoring) => self.finish(result, generation),
            Event::ReleaseTimedOut(attempt)
                if self.switch_attempt == attempt && matches!(self.pending, Some(Pending::Releasing(_))) =>
            {
                self.finish(Err(RELEASE_FAILED.into()), generation)
            }
            Event::RestoreTimedOut(attempt) if self.switch_attempt == attempt => {
                self.keyboard_restored();
                Task::none()
            }
            Event::Back if !self.retains_focus() => {
                self.menu = None;
                crate::input::focus(self.input())
            }
            _ if !self.interactive() => Task::none(),
            event => self.apply_input(event, generation),
        }
    }

    fn apply_input(&mut self, event: Event, generation: u64) -> Task<Message> {
        match event {
            Event::OpenActions(id) => self.open_actions(id),
            Event::Query(query) if self.menu.is_none() => self.change_query(query),
            Event::Press(id) if self.menu.is_none() => self.press(id),
            Event::DoubleClick(id) if self.press.take() == Some((id, true)) => {
                return self.run(id, Action::Switch, generation);
            }
            Event::Apply(id, action)
                if matches!(
                    self.menu.and_then(|choice| self.menu_action(choice)),
                    Some(Event::Run(target, current)) if target == id && current == action
                ) =>
            {
                return self.run(id, action, generation);
            }
            Event::Run(id, action) => return self.run(id, action, generation),
            _ => {}
        }
        Task::none()
    }

    fn open_actions(&mut self, id: u64) {
        if self.select(id) {
            self.menu = Some(0);
            self.press = None;
            self.confirmation = None;
            self.error = None;
        }
    }

    fn finish(&mut self, result: Result<(), String>, generation: u64) -> Task<Message> {
        let Some(pending) = self.pending.take() else { return Task::none() };
        match result {
            Ok(()) => {
                if let Pending::Applying(action) = pending {
                    self.confirmation = Some(action.confirmation());
                }
            }
            Err(error) => self.error = Some(error),
        }
        if pending.switching() {
            // A leave for the release request may still be queued when its timer fires.
            self.pending = Some(Pending::Restoring);
            return Task::done(Message::KeyboardInteractivityChange(KeyboardInteractivity::Exclusive))
                .chain(crate::input::focus(self.input()))
                .chain(keyboard_timeout(generation, Event::RestoreTimedOut(self.switch_attempt)));
        }
        Task::none()
    }

    fn notice(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if let Some(pending) = self.pending {
            return if pending.switching() {
                "Switching windows…".into()
            } else {
                "Waiting for the desktop…".into()
            };
        }
        if let Some(confirmation) = self.confirmation {
            return confirmation.into();
        }
        match self.windows.as_deref() {
            None => "Reading open windows…".into(),
            Some([]) => "No open windows. Open an application to switch to it.".into(),
            Some(_) => self.results_notice(),
        }
    }

    fn results_notice(&self) -> String {
        if self.selected_closed {
            return "That window is gone. Choose another window.".into();
        }
        match self.matches.len() {
            0 => "No matching windows. Try another search.".into(),
            1 => "1 window".into(),
            count => format!("{count} windows"),
        }
    }

    pub(super) fn view(&self, generation: u64, action_key: String) -> Element<'_, Message> {
        let back = button(text("Back").size(12)).on_press_maybe((!self.retains_focus()).then_some(Event::Back));
        let heading = if self.menu.is_some() { "Window actions" } else { "Switch windows" };
        let title = row![back.padding([6, 8]).style(status_button_style), text(heading).size(20)]
            .align_y(iced::Center)
            .spacing(12);
        let (field, items) = match self.menu {
            Some(choice) => (self.target_label(), self.action_list(choice)),
            None => (self.filter(), self.list()),
        };
        let body = column![
            page_header(title).height(56),
            divider(),
            container(field).padding([4, 8]),
            items,
            divider(),
            self.footer(action_key),
        ];
        let view = container(body).padding(PAD).width(Length::Fill).height(Length::Fill).style(surface_style);
        crate::shortcuts::capture_numbered(Element::from(view).map(move |event| Message::Windows { generation, event }))
    }

    fn filter(&self) -> Element<'_, Event> {
        text_input("Search by window title or app…", &self.query)
            .id(self.input())
            .on_input_maybe(self.interactive().then_some(Event::Query))
            .padding([6, 8])
            .style(field_style)
            .into()
    }

    fn target_label(&self) -> Element<'_, Event> {
        let title = self.selected_entry().map_or("", window_title);
        container(row_label(title, 15, palette::TEXT)).padding([6, 10]).into()
    }

    fn notice_label(&self) -> Element<'_, Event> {
        let role = if self.error.is_some() { palette::ERROR } else { palette::DIM };
        container(
            text(self.notice()).size(12).style(move |theme: &Theme| text::Style { color: Some(role.resolve(theme)) }),
        )
        .padding([4, 10])
        .into()
    }

    fn list(&self) -> Element<'_, Event> {
        let list = self.cursor.window(self.matches.len()).enumerate().fold(column![], |rows, (slot, index)| {
            let entry = &self.windows()[self.matches[index]];
            let selected = !self.selected_closed && index == self.cursor.selected;
            rows.push(entry.view(slot, selected, self.interactive()))
        });
        column![self.notice_label(), list].height(Length::Fill).padding([0, 4]).into()
    }

    fn action_list(&self, choice: usize) -> Element<'_, Event> {
        let mut rows = column![self.notice_label()].height(Length::Fill).padding([0, 4]);
        if let Some(entry) = self.selected_entry() {
            for (slot, action) in window_actions(entry).into_iter().enumerate() {
                let selected = slot == choice;
                let content = row![
                    text(action.label()).size(15),
                    space::horizontal(),
                    numbered_slot::<Event>(slot, selected, self.interactive()),
                ]
                .align_y(iced::Center)
                .padding([0, 10]);
                let row = button(selectable_result_row(content, selected))
                    .on_press_maybe(self.interactive().then_some(Event::Run(entry.id, action)))
                    .padding(0)
                    .style(|theme: &Theme, _| button::Style {
                        text_color: palette::TEXT.resolve(theme),
                        ..Default::default()
                    });
                rows = rows.push(row);
            }
        }
        rows.into()
    }

    fn footer(&self, action_key: String) -> Element<'_, Event> {
        let back = footer_button(hint("Esc", "Back"), (!self.retains_focus()).then_some(Event::Back));
        let mut actions = row![back, space::horizontal()].align_y(iced::Center);
        if let Some(id) = self.selected().filter(|_| self.interactive()) {
            actions = actions.extend(self.footer_actions(id, action_key));
        }
        container(actions).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style).into()
    }

    fn footer_actions(&self, id: u64, action_key: String) -> Vec<Element<'_, Event>> {
        match self.menu {
            Some(choice) => {
                let Some(Event::Run(id, action)) = self.menu_action(choice) else { return Vec::new() };
                vec![keyed_footer_button((id, Some(action)), primary_hint("Enter", "Apply"), Event::Apply(id, action))]
            }
            None => vec![
                keyed_footer_button((id, None::<Action>), hint(action_key, "Actions"), Event::OpenActions(id)),
                keyed_footer_button(
                    (id, Some(Action::Switch)),
                    primary_hint("Enter", "Switch"),
                    Event::Run(id, Action::Switch),
                ),
            ],
        }
    }
}

fn window_actions(entry: &Entry) -> [Action; 2] {
    [Action::Maximize(!entry.maximized), Action::Minimize(!entry.minimized)]
}

fn window_title(entry: &Entry) -> &str {
    if entry.title.trim().is_empty() { "Untitled window" } else { &entry.title }
}

struct WindowRow {
    entry: Entry,
    label: String,
    icon: Option<Icon>,
    search: [String; 3],
}

impl WindowRow {
    fn new(entry: Entry, apps: &[App], icons: &mut Icons) -> Self {
        let known = apps
            .iter()
            .find(|app| app.id == entry.app_id || app.id.strip_suffix(".desktop") == Some(entry.app_id.as_str()));
        let name = known.map(|app| app.name.as_str()).unwrap_or(&entry.app_id);
        let app = if name.is_empty() { "Unknown application" } else { name };
        let state = match (entry.minimized, entry.maximized) {
            (true, _) => " · Minimized",
            (_, true) => " · Maximized",
            _ => "",
        };
        let label = format!("{app}{state}");
        let icon = icons.get(
            known.and_then(|app| app.icon.as_deref()).unwrap_or_default(),
            &["focus-windows", "application-x-executable"],
        );
        let search = [&entry.title[..], &entry.app_id, app].map(crate::matcher::fold);
        Self { entry, label, icon, search }
    }

    fn matches(&self, terms: &[String]) -> bool {
        terms.iter().all(|term| self.search.iter().any(|field| field.contains(term)))
    }

    fn view(&self, slot: usize, selected: bool, interactive: bool) -> Element<'_, Event> {
        let title = window_title(&self.entry);
        let icon: Element<'_, Event> = match self.icon.clone() {
            Some(Icon::Svg(handle)) => svg(handle).width(ICON_SIZE).height(ICON_SIZE).into(),
            Some(Icon::Raster(handle)) => image(handle).width(ICON_SIZE).height(ICON_SIZE).into(),
            None => space().width(ICON_SIZE).height(ICON_SIZE).into(),
        };
        let content = row![
            icon,
            column![row_label(title, 15, palette::TEXT), row_label(self.label.as_str(), 12, palette::DIM)]
                .width(Length::Fill)
                .spacing(2),
            numbered_slot(slot, selected, interactive),
        ]
        .spacing(12)
        .align_y(iced::Center);
        let row = selectable_result_row(content.padding([0, 10]), selected);
        if !interactive {
            return row;
        }
        mouse_area(row)
            .on_press(Event::Press(self.entry.id))
            .on_double_click(Event::DoubleClick(self.entry.id))
            .on_right_press(Event::OpenActions(self.entry.id))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    }
}

fn row_label<'a>(label: impl Into<std::borrow::Cow<'a, str>>, size: u32, role: palette::Role) -> Element<'a, Event> {
    let label = label.into();
    responsive(move |area| {
        text(label_for_width(&label, area.width, size as f32, false))
            .size(size)
            .wrapping(text::Wrapping::None)
            .style(move |theme: &Theme| text::Style { color: Some(role.resolve(theme)) })
            .into()
    })
    .height(Length::Shrink)
    .into()
}

fn keyboard_timeout(generation: u64, event: Event) -> Task<Message> {
    Task::perform(
        async {
            if let Ok(runtime) = zruntime::SharedRuntime::current() {
                runtime.sleep(KEYBOARD_TIMEOUT).await;
            }
        },
        move |()| Message::Windows { generation, event: event.clone() },
    )
}

impl Launcher {
    pub(super) fn show_windows(&mut self) -> Task<Message> {
        self.panel = None;
        self.clicks = Default::default();
        self.window_switcher.generation += 1;
        let generation = self.window_switcher.generation;
        let mut panel = Panel::new();
        let task = match Client::start() {
            Ok((session, updates)) => {
                panel.session = Some(session);
                Task::run(updates, move |update| Message::Windows { generation, event: Event::Update(update) })
            }
            Err(message) => {
                panel.error = Some(message);
                Task::none()
            }
        };
        let focus = crate::input::focus(panel.input());
        self.window_switcher.panel = Some(panel);
        Task::batch([task, focus, self.resize()])
    }

    pub(super) fn update_windows(&mut self, message: Message) -> Task<Message> {
        let generation = self.window_switcher.generation;
        let panel = self.window_switcher.panel.as_mut().expect("window switcher is open");
        if panel.routes_to_parent(&message) {
            return update_content(self, message);
        }
        match message {
            Message::FocusLost => return panel.keyboard_released(generation),
            Message::FocusGained => {
                panel.keyboard_restored();
                return Task::none();
            }
            _ => {}
        }
        match panel.input_event(message, generation) {
            Some(Event::Back) if panel.menu.is_none() && !panel.retains_focus() => {
                self.window_switcher.panel = None;
                Task::batch([self.resize(), crate::input::focus(search_id())])
            }
            Some(Event::Finished(Ok(()))) if panel.pending == Some(Pending::Applying(Action::Switch)) => iced::exit(),
            Some(Event::Update(update)) => panel.receive(update, &self.apps, &mut self.icons),
            Some(event) => panel.event(event, generation),
            None => Task::none(),
        }
    }
}

impl Panel {
    fn routes_to_parent(&self, message: &Message) -> bool {
        match message {
            Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_) => true,
            Message::FocusLost => !self.retains_focus(),
            _ => false,
        }
    }

    fn input_event(&mut self, message: Message, generation: u64) -> Option<Event> {
        match message {
            Message::Windows { generation: current, event } if current == generation => Some(event),
            Message::Cmd(Command::Back) if !self.retains_focus() => Some(Event::Back),
            Message::Cmd(command) if self.interactive() => self.command(command),
            Message::Wheel(delta) if self.interactive() && self.menu.is_none() => {
                self.move_by(-(delta.signum() as isize));
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64, title: &str) -> Entry {
        Entry { id, title: title.into(), app_id: "org.example.Editor".into(), ..Entry::default() }
    }

    fn refresh(panel: &mut Panel, entries: Vec<Entry>) {
        let _ = panel.receive(Update::Windows(entries), &[], &mut Icons::disabled());
    }

    #[test]
    fn window_list_keeps_identity_across_renames_and_requires_selection_after_close() {
        let mut panel = Panel::new();
        refresh(&mut panel, vec![window(1, "Report"), window(2, "Report"), window(3, "Notes")]);
        panel.press(2);
        refresh(&mut panel, vec![window(2, "Renamed report"), window(3, "Notes")]);
        assert_eq!(panel.selected(), Some(2));

        refresh(&mut panel, vec![window(3, "Notes")]);
        refresh(&mut panel, vec![window(3, "Notes"), window(4, "Renamed report")]);
        assert!(panel.command(Command::Activate).is_none());
        assert!(!panel.select(2));
        assert!(matches!(panel.command(Command::ActivateVisible(1)), Some(Event::Run(4, Action::Switch))));
    }

    #[test]
    fn each_filter_term_can_match_the_title_or_application() {
        let mut panel = Panel::new();
        let entries =
            vec![window(1, "Résumé – report"), Entry { app_id: "org.example.Terminal".into(), ..window(2, "Report") }];
        refresh(&mut panel, entries);
        panel.change_query("REPORT editor".into());
        assert_eq!(panel.matches.len(), 1);
        assert_eq!(panel.selected(), Some(1));
    }

    #[test]
    fn window_actions_follow_confirmed_state_and_keep_selection() {
        let mut panel = Panel::new();
        refresh(&mut panel, vec![window(1, "Report"), window(2, "Notes")]);
        panel.select(2);
        panel.menu = Some(0);
        assert!(matches!(panel.command(Command::Activate), Some(Event::Run(2, Action::Maximize(true)))));
        refresh(&mut panel, vec![Entry { maximized: true, ..window(2, "Notes") }]);
        assert!(matches!(panel.command(Command::Activate), Some(Event::Run(2, Action::Maximize(false)))));
        refresh(&mut panel, vec![window(1, "Report")]);
        assert!(panel.menu.is_none());
        assert!(panel.command(Command::Activate).is_none());
    }

    #[test]
    fn window_footer_cancels_a_press_when_its_target_or_action_changes() {
        use iced::{Event as Input, Point, Size, mouse};
        use iced_runtime::{UserInterface, user_interface::Cache};

        let mut panel = Panel::new();
        refresh(&mut panel, vec![window(1, "Report"), window(2, "Notes")]);
        let press = Input::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Input::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let mut cache = Cache::default();
        let mut renderer =
            iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
        let mut send = |panel: &Panel, events: &[Input]| {
            let footer = row![space::horizontal()]
                .extend(panel.footer_actions(panel.selected().unwrap(), "Ctrl+K".into()))
                .align_y(iced::Center)
                .height(FOOTER_HEIGHT);
            let mut ui = UserInterface::build(
                footer,
                Size::new(400.0, FOOTER_HEIGHT),
                std::mem::take(&mut cache),
                &mut renderer,
            );
            let mut messages = Vec::new();
            ui.update(
                events,
                mouse::Cursor::Available(Point::new(380.0, FOOTER_HEIGHT / 2.0)),
                &mut renderer,
                &mut iced::advanced::clipboard::Null,
                &mut messages,
            );
            cache = ui.into_cache();
            messages
        };

        assert!(send(&panel, std::slice::from_ref(&press)).is_empty());
        assert!(panel.command(Command::Down).is_none());
        assert_eq!(panel.selected(), Some(2));
        let switched = send(&panel, std::slice::from_ref(&release));
        assert!(matches!(send(&panel, &[press.clone(), release.clone()]).as_slice(), [Event::Run(2, Action::Switch)]));

        panel.menu = Some(0);
        assert!(send(&panel, std::slice::from_ref(&press)).is_empty());
        refresh(&mut panel, vec![window(1, "Report"), Entry { maximized: true, ..window(2, "Notes") }]);
        let applied = send(&panel, std::slice::from_ref(&release));
        let mut queued = send(&panel, &[press, release]);
        assert!(matches!(queued.as_slice(), [Event::Apply(2, Action::Maximize(false))]));
        assert!(switched.is_empty() && applied.is_empty(), "stale releases dispatched {switched:?} and {applied:?}");
        // A live update may also be delivered ahead of an already queued click.
        refresh(&mut panel, vec![window(1, "Report"), window(2, "Notes")]);
        panel.confirmation = Some("Window size restored");
        let _ = panel.apply_input(queued.remove(0), 1);
        assert_eq!(panel.notice(), "Window size restored", "the queued inverse action must not run");
    }

    #[test]
    fn focus_loss_dismisses_window_state_actions_only() {
        let mut panel = Panel::new();
        panel.pending = Some(Pending::Applying(Action::Maximize(true)));
        assert!(panel.routes_to_parent(&Message::FocusLost));
        panel.pending = Some(Pending::Releasing(2));
        assert!(!panel.routes_to_parent(&Message::FocusLost));
        panel.pending = Some(Pending::Applying(Action::Switch));
        assert!(!panel.routes_to_parent(&Message::FocusLost));
    }

    #[test]
    fn a_switch_failure_restores_input_and_preserves_the_filter_and_target() {
        use iced::futures::{StreamExt, executor::block_on};
        use iced_runtime::Action as RuntimeAction;

        for (pending, event) in [
            (Pending::Releasing(2), Event::ReleaseTimedOut(1)),
            (Pending::Applying(Action::Switch), Event::Finished(Err("Activation unconfirmed".into()))),
        ] {
            let mut panel = Panel::new();
            refresh(&mut panel, vec![window(1, "Report"), window(2, "Notes")]);
            panel.change_query("notes".into());
            panel.pending = Some(pending);
            panel.switch_attempt = 1;
            let stale = panel.event(Event::ReleaseTimedOut(0), 1);
            assert!(iced_runtime::task::into_stream(stale).is_none());
            assert!(panel.retains_focus());

            let restore = panel.event(event, 1);
            let mut actions = iced_runtime::task::into_stream(restore).unwrap();
            assert!(matches!(
                block_on(actions.next()),
                Some(RuntimeAction::Output(Message::KeyboardInteractivityChange(KeyboardInteractivity::Exclusive)))
            ));
            assert!(matches!(block_on(actions.next()), Some(RuntimeAction::Widget(_))));
            assert!(panel.pending == Some(Pending::Restoring));
            assert_eq!(panel.query, "notes");
            assert_eq!(panel.selected(), Some(2));
            assert!(panel.error.is_some());
        }
    }

    #[test]
    fn a_late_keyboard_leave_after_release_timeout_keeps_the_panel_open() {
        let mut panel = Panel::new();
        panel.pending = Some(Pending::Releasing(2));
        panel.switch_attempt = 1;
        let _ = panel.event(Event::ReleaseTimedOut(1), 1);
        assert!(!panel.routes_to_parent(&Message::FocusLost));
        let _ = panel.event(Event::RestoreTimedOut(0), 1);
        assert!(!panel.routes_to_parent(&Message::FocusLost));
        panel.keyboard_restored();
        assert!(panel.routes_to_parent(&Message::FocusLost));
    }

    #[test]
    fn window_helper_disconnect_keeps_the_last_complete_selection() {
        let mut panel = Panel::new();
        refresh(&mut panel, vec![window(1, "Report"), window(2, "Notes")]);
        panel.change_query("notes".into());
        let _ = panel.receive(Update::Unavailable("Disconnected".into()), &[], &mut Icons::disabled());
        assert_eq!(panel.query, "notes");
        assert_eq!(panel.selected(), Some(2));
        assert_eq!(panel.error.as_deref(), Some("Disconnected"));
    }
}
