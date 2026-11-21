// SPDX-License-Identifier: GPL-3.0-or-later
//! Search open windows without changing the parent search.

use iced::widget::{Id, button, column, container, image, mouse_area, responsive, row, space, svg, text, text_input};
use iced::{Element, Length, Task, Theme};

use super::{
    ICON_SIZE, Launcher, Message, divider, field_style, footer_style, hint, label_for_width, numbered_slot,
    page_header, palette, primary_hint, search_id, selectable_result_row, status_button_style, surface_style,
    update_content,
};
use crate::apps::App;
use crate::icons::{Icon, Icons};
use crate::layout::{FOOTER_HEIGHT, PAD, ROW_HEIGHT};
use crate::model::{Command, Cursor, NUMBERED_SLOTS};
use crate::window_switcher::{Entry, Session, Update};

const LIST_CHROME: u32 = 172;
pub(super) const HEIGHT: u32 = LIST_CHROME + ROW_HEIGHT as u32 * 5;

#[derive(Debug, Clone)]
pub enum Event {
    Update(Update),
    Query(String),
    Press(u64),
    DoubleClick(u64),
    Switch(u64),
    Switched(Result<(), &'static str>),
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
    session: Option<Session>,
    switching: bool,
    error: Option<&'static str>,
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
            switching: false,
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
        if self.selected_closed {
            return None;
        }
        self.matches.get(self.cursor.selected).map(|&index| self.windows()[index].entry.id)
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

    fn receive(&mut self, update: Update, apps: &[App], icons: &mut Icons) {
        match update {
            Update::Windows(windows) => {
                let selection = self.selected();
                self.windows = Some(windows.into_iter().map(|entry| WindowRow::new(entry, apps, icons)).collect());
                let closed = self.selected_closed;
                self.refresh_matches(selection);
                self.selected_closed |= closed;
            }
            Update::Unavailable(message) => {
                self.windows = None;
                self.matches.clear();
                self.session = None;
                self.error = Some(message);
            }
        }
    }

    fn change_query(&mut self, query: String) {
        self.query = query;
        self.cursor.reset();
        self.press = None;
        self.error = None;
        self.refresh_matches(None);
    }

    fn press(&mut self, id: u64) {
        if self.select(id) {
            self.press = Some((id, self.press.is_some_and(|(previous, _)| previous == id)));
        }
    }

    fn command(&mut self, command: Command) -> Option<Event> {
        let page = self.cursor.visible as isize;
        let delta = match command {
            Command::Up => -1,
            Command::Down => 1,
            Command::PageUp => -page,
            Command::PageDown => page,
            Command::First => -(self.cursor.selected as isize),
            Command::Last => self.matches.len() as isize,
            Command::Activate => return self.selected().map(Event::Switch),
            Command::ActivateVisible(slot) => {
                if self.cursor.select_visible(slot, self.matches.len()) {
                    self.selected_closed = false;
                    return self.selected().map(Event::Switch);
                }
                return None;
            }
            Command::Back => return Some(Event::Back),
            _ => return None,
        };
        self.move_by(delta);
        None
    }

    fn move_by(&mut self, delta: isize) {
        self.cursor.move_by(delta, self.matches.len());
        self.selected_closed = false;
        self.press = None;
    }

    fn switch(&mut self, id: u64) -> Task<Event> {
        let Some(session) = &self.session else { return Task::none() };
        if self.selected() != Some(id) {
            return Task::none();
        }
        self.switching = true;
        self.error = None;
        Task::perform(session.activate(id), Event::Switched)
    }

    fn event(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Switched(Err(error)) => {
                self.switching = false;
                self.error = Some(error);
            }
            _ if self.switching => {}
            Event::Query(query) if self.session.is_some() => self.change_query(query),
            Event::Press(id) => self.press(id),
            Event::DoubleClick(id) if self.press.take() == Some((id, true)) => return self.switch(id),
            Event::Switch(id) => return self.switch(id),
            _ => {}
        }
        Task::none()
    }

    fn notice(&self) -> String {
        if let Some(error) = self.error {
            return error.into();
        }
        if self.switching {
            return "Switching windows…".into();
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

    pub(super) fn view(&self, generation: u64) -> Element<'_, Message> {
        let back = button(text("Back").size(12)).on_press_maybe((!self.switching).then_some(Event::Back));
        let title = row![back.padding([6, 8]).style(status_button_style), text("Switch windows").size(20)]
            .align_y(iced::Center)
            .spacing(12);
        let filter = text_input("Search by window title or app…", &self.query)
            .id(self.input())
            .on_input_maybe(self.session.as_ref().filter(|_| !self.switching).map(|_| Event::Query))
            .padding([6, 8])
            .style(field_style);
        let body = column![
            page_header(title).height(56),
            divider(),
            container(filter).padding([4, 8]),
            self.list(),
            divider(),
            self.footer(),
        ];
        let view = container(body).padding(PAD).width(Length::Fill).height(Length::Fill).style(surface_style);
        crate::shortcuts::capture_numbered(Element::from(view).map(move |event| Message::Windows { generation, event }))
    }

    fn list(&self) -> Element<'_, Event> {
        let role = if self.error.is_some() { palette::ERROR } else { palette::DIM };
        let notice =
            text(self.notice()).size(12).style(move |theme: &Theme| text::Style { color: Some(role.resolve(theme)) });
        let list = self.cursor.window(self.matches.len()).enumerate().fold(column![], |rows, (slot, index)| {
            let entry = &self.windows()[self.matches[index]];
            let selected = !self.selected_closed && index == self.cursor.selected;
            rows.push(entry.view(slot, selected, self.switching))
        });
        column![container(notice).padding([4, 10]), list].height(Length::Fill).padding([0, 4]).into()
    }

    fn footer(&self) -> Element<'_, Event> {
        let back = button(hint("Esc", "Back"))
            .on_press_maybe((!self.switching).then_some(Event::Back))
            .padding([4, 6])
            .style(status_button_style);
        let mut actions = row![back, space::horizontal()].align_y(iced::Center);
        if let Some(id) = self.selected().filter(|_| !self.switching) {
            actions = actions.push(
                button(primary_hint("Enter", "Switch"))
                    .on_press(Event::Switch(id))
                    .padding([4, 6])
                    .style(status_button_style),
            );
        }
        container(actions).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style).into()
    }
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
        let label = if entry.minimized { format!("{app} · Minimized") } else { app.into() };
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

    fn view(&self, slot: usize, selected: bool, switching: bool) -> Element<'_, Event> {
        let title = if self.entry.title.trim().is_empty() { "Untitled window" } else { &self.entry.title };
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
            numbered_slot(slot, selected, !switching),
        ]
        .spacing(12)
        .align_y(iced::Center);
        let row = selectable_result_row(content.padding([0, 10]), selected);
        if switching {
            return row;
        }
        mouse_area(row)
            .on_press(Event::Press(self.entry.id))
            .on_double_click(Event::DoubleClick(self.entry.id))
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

impl Launcher {
    pub(super) fn show_windows(&mut self) -> Task<Message> {
        self.panel = None;
        self.clicks = Default::default();
        self.window_switcher.generation += 1;
        let generation = self.window_switcher.generation;
        let mut panel = Panel::new();
        let task = match Session::start() {
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
        if matches!(&message, Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_)) {
            return update_content(self, message);
        }
        let generation = self.window_switcher.generation;
        let panel = self.window_switcher.panel.as_mut().expect("window switcher is open");
        if matches!(message, Message::FocusLost) && !panel.switching {
            return update_content(self, message);
        }
        match panel.input_event(message, generation) {
            Some(Event::Back) if !panel.switching => {
                self.window_switcher.panel = None;
                Task::batch([self.resize(), crate::input::focus(search_id())])
            }
            Some(Event::Switched(Ok(()))) => iced::exit(),
            Some(Event::Update(update)) => {
                panel.receive(update, &self.apps, &mut self.icons);
                Task::none()
            }
            Some(event) => panel.event(event).map(move |event| Message::Windows { generation, event }),
            None => Task::none(),
        }
    }
}

impl Panel {
    fn input_event(&mut self, message: Message, generation: u64) -> Option<Event> {
        match message {
            Message::Windows { generation: current, event } if current == generation => Some(event),
            Message::Cmd(command) if !self.switching => self.command(command),
            Message::Wheel(delta) if !self.switching => {
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
        Entry { id, title: title.into(), app_id: "org.example.Editor".into(), minimized: false }
    }

    fn refresh(panel: &mut Panel, entries: Vec<Entry>) {
        panel.receive(Update::Windows(entries), &[], &mut Icons::disabled());
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
        assert!(matches!(panel.command(Command::ActivateVisible(1)), Some(Event::Switch(4))));
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
}
