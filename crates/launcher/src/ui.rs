// SPDX-License-Identifier: GPL-3.0-or-later
//! The launcher window: an iced program on a wlr-layer-shell overlay.

use std::sync::Mutex;

use iced::alignment::{Horizontal, Vertical};
use iced::event::{self, Event};
use iced::futures::channel::mpsc::UnboundedReceiver;
use iced::futures::stream::{self, BoxStream, StreamExt};
use iced::keyboard::{self, Key, Modifiers};
use iced::mouse::{self, ScrollDelta};
use iced::widget::{self, Id, column, container, image, mouse_area, row, space, stack, svg, text, text_input};
use iced::{Background, Border, Color, Element, Length, Subscription, Task, Theme};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};
use iced_layershell::to_layer_message;

use crate::apps::App;
use crate::backend::{ACTION_OPEN_PARENT, Backend, FileHit, UiEvent};
use crate::icons::{Icon, Icons};
use crate::model::{self, Action, Command, Cursor, Item, Target};
use crate::usage::{self, Kind, Usage};

pub const WIDTH: u32 = 720;
const VISIBLE_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 48.0;
const INPUT_HEIGHT: f32 = 58.0;
const FOOTER_HEIGHT: f32 = 34.0;
const PAD: f32 = 8.0;
const ICON_SIZE: f32 = 28.0;
/// Below this, file search is not run (the daemon needs two characters).
const MIN_FILE_QUERY: usize = 2;

fn compact_height() -> u32 {
    (PAD * 2.0 + INPUT_HEIGHT + 1.0 + FOOTER_HEIGHT) as u32
}

fn full_height() -> u32 {
    compact_height() + (ROW_HEIGHT * VISIBLE_ROWS as f32 + PAD) as u32
}

mod palette {
    use iced::Color;
    pub const SURFACE: Color = Color::from_rgba8(0x17, 0x18, 0x1c, 0.97);
    pub const BORDER: Color = Color::from_rgba8(0xff, 0xff, 0xff, 0.10);
    pub const TEXT: Color = Color::from_rgb8(0xec, 0xea, 0xe6);
    pub const DIM: Color = Color::from_rgb8(0x9b, 0x9d, 0xa6);
    pub const FAINT: Color = Color::from_rgba8(0xff, 0xff, 0xff, 0.06);
    /// Oxidised iron.
    pub const ACCENT: Color = Color::from_rgb8(0xe0, 0x70, 0x3a);
    pub const SELECTED: Color = Color::from_rgba8(0xe0, 0x70, 0x3a, 0.18);
    pub const ERROR: Color = Color::from_rgb8(0xe8, 0x6a, 0x6a);
}

static UI_EVENTS: Mutex<Option<UnboundedReceiver<UiEvent>>> = Mutex::new(None);
static BOOT: Mutex<Option<Boot>> = Mutex::new(None);

/// What the launcher starts with.
pub struct Boot {
    pub backend: Backend,
    pub apps: Vec<App>,
    pub usage: Usage,
    pub initial_query: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Idle,
    Searching,
    Error(String),
    Notice(String),
}

struct Panel {
    title: String,
    actions: Vec<Action>,
    query: String,
    selected: usize,
}

impl Panel {
    fn new(item: &Item) -> Self {
        Self { title: item.title.clone(), actions: model::actions_for(&item.target), query: String::new(), selected: 0 }
    }

    fn filtered_actions(&self) -> Vec<Action> {
        let tokens: Vec<_> = self.query.split_whitespace().map(str::to_lowercase).collect();
        self.actions.iter().copied().filter(|a| tokens.iter().all(|t| a.label().to_lowercase().contains(t))).collect()
    }

    fn filter(&mut self, query: String) {
        self.query = query;
        self.selected = 0;
    }

    fn action_for_command(&self, cmd: Command) -> Option<Action> {
        match cmd {
            Command::Activate => self.filtered_actions().get(self.selected).copied(),
            Command::Secondary => self.actions.contains(&Action::Reveal).then_some(Action::Reveal),
            Command::CopyPath => self.actions.contains(&Action::CopyPath).then_some(Action::CopyPath),
            _ => None,
        }
    }
}

pub struct Launcher {
    backend: Backend,
    apps: Vec<App>,
    usage: Usage,
    /// The file being opened by the daemon. While set, further opens are
    /// ignored; its use is recorded once the daemon confirms.
    opening: Option<(String, String)>,
    icons: Icons,
    query: String,
    app_hits: Vec<(usize, u32)>,
    files: Vec<FileHit>,
    items: Vec<Item>,
    /// Resolved icon per item, filled when the list is rebuilt.
    item_icons: Vec<Option<Icon>>,
    cursor: Cursor,
    panel: Option<Panel>,
    generation: u64,
    status: Status,
    compact: bool,
}

#[to_layer_message]
#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    ActionQuery(String),
    Files { generation: u64, result: Result<Option<Vec<FileHit>>, String> },
    Cmd(Command),
    Wheel(f32),
    Clicked(usize),
    PanelClicked(usize),
    Opened(Result<(), String>),
    Ui(UiEvent),
}

fn search_id() -> Id {
    Id::new("search")
}

fn actions_id() -> Id {
    Id::new("action-search")
}

pub fn run(boot_data: Boot, events: UnboundedReceiver<UiEvent>) -> Result<(), iced_layershell::Error> {
    *UI_EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(events);
    *BOOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(boot_data);
    iced_layershell::application(boot, namespace, update, view)
        .style(|_, _| iced::theme::Style { background_color: Color::TRANSPARENT, text_color: palette::TEXT })
        .theme(|_: &Launcher| Theme::Dark)
        .subscription(subscription)
        .settings(Settings {
            layer_settings: LayerShellSettings {
                anchor: Anchor::Top,
                layer: Layer::Overlay,
                exclusive_zone: 0,
                size: Some((WIDTH, compact_height())),
                // About a fifth down a 1080p screen; layer-shell cannot
                // centre vertically without filling the output.
                margin: (200, 0, 0, 0),
                keyboard_interactivity: KeyboardInteractivity::Exclusive,
                start_mode: StartMode::Active,
                events_transparent: false,
            },
            antialiasing: true,
            default_text_size: 15.into(),
            ..Default::default()
        })
        .run()
}

fn namespace() -> String {
    "ferroseek-launcher".to_string()
}

fn boot() -> (Launcher, Task<Message>) {
    let Boot { backend, apps, usage, initial_query } =
        BOOT.lock().unwrap_or_else(|e| e.into_inner()).take().expect("boot data set once by run()");
    let mut state = Launcher {
        backend,
        apps,
        usage,
        opening: None,
        icons: Icons::new(ICON_SIZE as u16),
        query: String::new(),
        app_hits: Vec::new(),
        files: Vec::new(),
        items: Vec::new(),
        item_icons: Vec::new(),
        cursor: Cursor::new(VISIBLE_ROWS),
        panel: None,
        generation: 0,
        status: Status::Idle,
        compact: true,
    };
    let prefill = if initial_query.is_empty() {
        Task::none()
    } else {
        Task::batch([Task::done(Message::Query(initial_query)), widget::operation::move_cursor_to_end(search_id())])
    };
    state.rebuild(false); // recently opened items for the empty query
    let size = state.resize();
    (state, Task::batch([widget::operation::focus(search_id()), size, prefill]))
}

fn subscription(_: &Launcher) -> Subscription<Message> {
    Subscription::batch([event::listen_with(on_event), Subscription::run(ui_events)])
}

fn on_event(event: Event, _status: event::Status, _window: iced::window::Id) -> Option<Message> {
    match event {
        // Read regardless of capture: the search field keeps focus and
        // swallows Escape, but navigation keys must still reach the list.
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => key_message(&key, modifiers),
        Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
            let dy = match delta {
                ScrollDelta::Lines { y, .. } => y,
                ScrollDelta::Pixels { y, .. } => y / ROW_HEIGHT,
            };
            (dy != 0.0).then_some(Message::Wheel(dy))
        }
        _ => None,
    }
}

fn key_message(key: &Key, mods: Modifiers) -> Option<Message> {
    model::command_for(key, mods).map(Message::Cmd)
}

fn ui_events() -> BoxStream<'static, Message> {
    match UI_EVENTS.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(rx) => rx.map(Message::Ui).boxed(),
        None => stream::pending().boxed(),
    }
}

impl Launcher {
    /// Recomputes the rows. With `keep_selection` (more results for the
    /// same query), the selected target and its action panel stay put
    /// instead of jumping to the first row.
    fn rebuild(&mut self, keep_selection: bool) {
        let previous = keep_selection.then(|| self.selected().map(|i| i.target.clone())).flatten();
        self.items = if self.query.trim().is_empty() {
            model::home_items(&self.apps, &self.usage.recent(usage::now(), usize::MAX), VISIBLE_ROWS)
        } else {
            let mut items: Vec<Item> = model::calc_item(&self.query).into_iter().collect();
            items.extend(model::build_items(&self.apps, &self.app_hits, &self.files));
            items
        };
        let icons = &mut self.icons;
        self.item_icons = self
            .items
            .iter()
            .map(|item| {
                let fallbacks: &[&str] = match item.target {
                    Target::App(_) => &["application-x-executable"],
                    Target::File(ref f) if f.is_dir => &["folder"],
                    Target::File(_) => &["text-x-generic", "unknown"],
                    Target::Calc(_) => &["accessories-calculator", "calc"],
                };
                icons.get(&item.icon, fallbacks)
            })
            .collect();
        let kept = previous.and_then(|t| self.items.iter().position(|i| i.target == t));
        match kept {
            Some(j) => self.cursor.select(j, self.items.len()),
            None => {
                self.cursor.reset();
                self.panel = None;
            }
        }
    }

    fn resize(&mut self) -> Task<Message> {
        // Not while searching: rows would collapse and reappear per key.
        let compact = self.query.trim().is_empty()
            && self.items.is_empty()
            && !matches!(self.status, Status::Error(_) | Status::Notice(_) | Status::Searching);
        if compact == self.compact {
            return Task::none();
        }
        self.compact = compact;
        let h = if compact { compact_height() } else { full_height() };
        Task::done(Message::SizeChange((WIDTH, h)))
    }

    fn selected(&self) -> Option<&Item> {
        self.items.get(self.cursor.selected)
    }

    fn perform(&mut self, action: Action) -> Task<Message> {
        let Some(item) = self.selected().cloned() else { return Task::none() };
        let opens = matches!(action, Action::Launch | Action::Open | Action::Reveal);
        if opens && self.opening.is_some() {
            return Task::none(); // one open at a time; the launcher exits after it
        }
        match (action, &item.target) {
            (Action::Launch, Target::App(i)) => match crate::launch::launch(&self.apps[*i]) {
                Ok(()) => {
                    self.usage.record(Kind::App, &self.apps[*i].id, &self.query, usage::now());
                    iced::exit()
                }
                Err(e) => {
                    self.status = Status::Error(format!("Cannot launch {}: {e}", item.title));
                    self.resize()
                }
            },
            (Action::Open | Action::Reveal, Target::File(f)) => {
                let act = if action == Action::Reveal { ACTION_OPEN_PARENT } else { "" };
                self.opening = Some((f.id.clone(), self.query.clone()));
                Task::perform(self.backend.clone().run(f.id.clone(), act), Message::Opened)
            }
            (Action::CopyPath, Target::File(f)) => match &f.path {
                Some(p) => self.copy(p.to_string_lossy().into_owned(), "path"),
                None => Task::none(),
            },
            (Action::CopyName, _) => self.copy(item.title.clone(), "name"),
            (Action::CopyResult, Target::Calc(v)) => self.copy(v.clone(), "result"),
            _ => Task::none(),
        }
    }

    /// Copies and stays open: on Wayland the clipboard is served by its
    /// owner, so exiting right away could lose it without a clipboard
    /// manager.
    fn copy(&mut self, s: String, what: &str) -> Task<Message> {
        self.status = Status::Notice(format!("Copied {what}"));
        self.panel = None;
        Task::batch([iced::clipboard::write(s), widget::operation::focus(search_id())])
    }

    fn primary(&self) -> Option<Action> {
        self.selected().and_then(|i| model::actions_for(&i.target).first().copied())
    }
}

fn update(s: &mut Launcher, message: Message) -> Task<Message> {
    match message {
        Message::Query(q) => {
            if s.panel.is_some() {
                return widget::operation::focus(actions_id());
            }
            s.query = q;
            s.generation += 1;
            let (usage, now) = (&s.usage, usage::now());
            let q = s.query.as_str();
            s.app_hits = model::match_apps(&s.apps, q, |a| usage.boost(Kind::App, &a.id, q, now));
            let file_query = s.query.trim().chars().count() >= MIN_FILE_QUERY;
            // Results of the previous query must not stay actionable while
            // this one is searched.
            s.files.clear();
            s.status = if file_query { Status::Searching } else { Status::Idle };
            s.rebuild(false);
            let search = if file_query {
                let generation = s.generation;
                Task::perform(s.backend.clone().search(s.query.clone()), move |result| Message::Files {
                    generation,
                    result,
                })
            } else {
                Task::none()
            };
            Task::batch([search, s.resize()])
        }
        Message::ActionQuery(q) => {
            if let Some(panel) = &mut s.panel {
                panel.filter(q);
            }
            Task::none()
        }
        Message::Files { generation, result } => {
            if generation != s.generation {
                return Task::none(); // a newer query is in flight
            }
            match result {
                Ok(Some(mut files)) => {
                    let (usage, now) = (&s.usage, usage::now());
                    let q = s.query.as_str();
                    model::order_files(&mut files, |f| usage.boost(Kind::File, &f.id, q, now));
                    s.files = files;
                    s.status = Status::Idle;
                }
                Ok(None) => return Task::none(),
                Err(e) => {
                    s.files.clear();
                    s.status = Status::Error(format!("File search unavailable: {e}"));
                }
            }
            let had_panel = s.panel.is_some();
            s.rebuild(true);
            let focus =
                if had_panel && s.panel.is_none() { widget::operation::focus(search_id()) } else { Task::none() };
            Task::batch([s.resize(), focus])
        }
        Message::Cmd(cmd) => command(s, cmd),
        Message::Wheel(dy) => {
            if s.panel.is_none() {
                s.cursor.move_by(if dy > 0.0 { -1 } else { 1 }, s.items.len());
            }
            Task::none()
        }
        Message::Clicked(i) => {
            if s.panel.is_some() {
                s.panel = None;
                return widget::operation::focus(search_id());
            }
            s.cursor.select(i, s.items.len());
            s.panel = None;
            match s.primary() {
                Some(a) => s.perform(a),
                None => Task::none(),
            }
        }
        Message::PanelClicked(i) => {
            let a = s.panel.as_ref().and_then(|p| p.filtered_actions().get(i).copied());
            a.map_or_else(Task::none, |a| s.perform(a))
        }
        Message::Opened(Ok(())) => {
            if let Some((id, query)) = s.opening.take() {
                s.usage.record(Kind::File, &id, &query, usage::now());
            }
            iced::exit()
        }
        Message::Opened(Err(e)) => {
            s.opening = None;
            s.status = Status::Error(format!("Cannot open: {e}"));
            s.resize()
        }
        Message::Ui(UiEvent::Toggle) => iced::exit(),
        // Layer-shell requests generated by `to_layer_message` are handled
        // by the runtime before they reach here.
        _ => Task::none(),
    }
}

fn command(s: &mut Launcher, cmd: Command) -> Task<Message> {
    let len = s.items.len();
    if let Some(p) = &mut s.panel {
        if let Some(action) = p.action_for_command(cmd) {
            return s.perform(action);
        }
        match cmd {
            Command::Up => p.selected = p.selected.saturating_sub(1),
            Command::Down => p.selected = (p.selected + 1).min(p.filtered_actions().len().saturating_sub(1)),
            Command::First | Command::PageUp => p.selected = 0,
            Command::Last | Command::PageDown => p.selected = p.filtered_actions().len().saturating_sub(1),
            Command::Back if !p.query.is_empty() => {
                p.filter(String::new());
                return widget::operation::focus(actions_id());
            }
            Command::Back | Command::ToggleActions => {
                s.panel = None;
                return widget::operation::focus(search_id());
            }
            _ => {}
        }
        return Task::none();
    }
    let page = VISIBLE_ROWS as isize - 1;
    match cmd {
        Command::Up => s.cursor.move_by(-1, len),
        Command::Down => s.cursor.move_by(1, len),
        Command::PreviousSection => s.cursor.move_section(false, &s.items),
        Command::NextSection => s.cursor.move_section(true, &s.items),
        Command::PageUp => s.cursor.move_by(-page, len),
        Command::PageDown => s.cursor.move_by(page, len),
        Command::First => s.cursor.move_by(-(len as isize), len),
        Command::Last => s.cursor.move_by(len as isize, len),
        Command::Activate => return s.primary().map_or_else(Task::none, |a| s.perform(a)),
        Command::Secondary => {
            let has_reveal = s.selected().is_some_and(|i| model::actions_for(&i.target).contains(&Action::Reveal));
            if has_reveal {
                return s.perform(Action::Reveal);
            }
        }
        Command::CopyPath => return s.perform(Action::CopyPath),
        Command::ToggleActions => {
            if let Some(item) = s.selected() {
                s.panel = Some(Panel::new(item));
                return widget::operation::focus(actions_id());
            }
        }
        Command::Back => {
            if s.query.is_empty() {
                return iced::exit();
            }
            // Escape also unfocuses the field; clear and take focus back.
            return Task::batch([Task::done(Message::Query(String::new())), widget::operation::focus(search_id())]);
        }
    }
    Task::none()
}

// ---------------------------------------------------------------- view

fn view(s: &Launcher) -> Element<'_, Message> {
    let input = text_input("Search applications and files…", &s.query)
        .id(search_id())
        .on_input(Message::Query)
        .size(20)
        .padding([14, 12])
        .style(|_, _| text_input::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            icon: palette::DIM,
            placeholder: palette::DIM,
            value: palette::TEXT,
            selection: palette::SELECTED,
        });
    let header = container(input).height(INPUT_HEIGHT).align_y(Vertical::Center);

    let mut body = column![header, divider()];
    if !s.compact {
        body =
            body.push(container(results(s)).height(ROW_HEIGHT * VISIBLE_ROWS as f32 + PAD).padding([PAD / 2.0, 0.0]));
        body = body.push(divider());
    }
    body = body.push(footer(s));

    let surface =
        container(body).padding([PAD, PAD]).width(Length::Fill).height(Length::Fill).style(|_| container::Style {
            background: Some(Background::Color(palette::SURFACE)),
            border: Border { color: palette::BORDER, width: 1.0, radius: 14.0.into() },
            text_color: Some(palette::TEXT),
            ..Default::default()
        });

    match &s.panel {
        Some(p) => stack![surface, action_panel(p)].into(),
        None => surface.into(),
    }
}

fn divider<'a>() -> Element<'a, Message> {
    container(space::horizontal())
        .height(1)
        .width(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(palette::FAINT)), ..Default::default() })
        .into()
}

fn results(s: &Launcher) -> Element<'_, Message> {
    if s.items.is_empty() {
        let (msg, help) = match &s.status {
            Status::Searching => ("Searching…", ""),
            Status::Error(_) => ("File search unavailable", "You can still search applications or calculate a result."),
            _ if s.query.trim().is_empty() => ("Start typing to search", "Find an application, a file, or try 2 + 3."),
            _ if s.query.trim().chars().count() < MIN_FILE_QUERY => {
                ("No matching applications", "Type at least 2 characters to search files.")
            }
            _ => ("No results", "Try a shorter name or a different keyword."),
        };
        return container(
            column![text(msg).size(18), text(help).size(13).color(palette::DIM)].spacing(8).align_x(Horizontal::Center),
        )
        .center(Length::Fill)
        .into();
    }
    let mut col = column![];
    for i in s.cursor.window(s.items.len()) {
        col = col.push(result_row(s, i));
    }
    col.into()
}

fn result_row(s: &Launcher, i: usize) -> Element<'_, Message> {
    let item = &s.items[i];
    let selected = i == s.cursor.selected;
    let icon: Element<'_, Message> = match s.item_icons.get(i).cloned().flatten() {
        Some(Icon::Svg(h)) => svg(h).width(ICON_SIZE).height(ICON_SIZE).into(),
        Some(Icon::Raster(h)) => image(h).width(ICON_SIZE).height(ICON_SIZE).into(),
        None => space().width(ICON_SIZE).height(ICON_SIZE).into(),
    };
    let mut labels = column![text(ellipsize_end(&item.title, 60)).size(15).wrapping(text::Wrapping::None)].spacing(1);
    if !item.subtitle.is_empty() {
        labels = labels.push(
            text(ellipsize_middle(&item.subtitle, 80)).size(12).color(palette::DIM).wrapping(text::Wrapping::None),
        );
    }
    let tag = container(text(item.tag).size(11).color(palette::DIM)).padding([2, 8]).style(|_| container::Style {
        background: Some(Background::Color(palette::FAINT)),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    });
    let content = row![icon, container(labels).width(Length::Fill).clip(true), tag]
        .spacing(12)
        .align_y(Vertical::Center)
        .padding([0, 12]);
    let bar = container(space::vertical()).width(3).height(ROW_HEIGHT - 16.0).style(move |_| container::Style {
        background: selected.then_some(Background::Color(palette::ACCENT)),
        border: Border { radius: 2.0.into(), ..Default::default() },
        ..Default::default()
    });
    let row = container(row![bar, content].align_y(Vertical::Center)).height(ROW_HEIGHT).width(Length::Fill).style(
        move |_| container::Style {
            background: selected.then_some(Background::Color(palette::SELECTED)),
            border: Border { radius: 9.0.into(), ..Default::default() },
            ..Default::default()
        },
    );
    mouse_area(row).on_press(Message::Clicked(i)).interaction(mouse::Interaction::Pointer).into()
}

fn footer(s: &Launcher) -> Element<'_, Message> {
    let (msg, color) = match &s.status {
        Status::Error(e) => (ellipsize_end(e, 70), palette::ERROR),
        Status::Notice(n) => (n.clone(), palette::ACCENT),
        Status::Searching => ("Searching…".to_string(), palette::DIM),
        Status::Idle if s.items.is_empty() && !s.query.trim().is_empty() => ("No results".to_string(), palette::DIM),
        Status::Idle if s.items.is_empty() => ("FerroSeek".to_string(), palette::DIM),
        Status::Idle if s.query.trim().is_empty() => ("Suggestions".to_string(), palette::DIM),
        Status::Idle => match s.items.len() {
            1 => ("1 result".to_string(), palette::DIM),
            n => (format!("{n} results"), palette::DIM),
        },
    };
    let mut hints = row![].spacing(14).align_y(Vertical::Center);
    if let Some(item) = s.selected() {
        let acts = model::actions_for(&item.target);
        hints = hints.push(hint("Enter", acts[0].label()));
        if acts.contains(&Action::Reveal) {
            hints = hints.push(hint("Ctrl+Enter", Action::Reveal.label()));
        }
        hints = hints.push(hint("Ctrl+K", "Actions"));
    } else {
        hints = hints.push(hint("Esc", if s.query.is_empty() { "Close" } else { "Clear search" }));
    }
    container(
        row![text(msg).size(12).color(color).wrapping(text::Wrapping::None), space::horizontal(), hints]
            .align_y(Vertical::Center)
            .padding([0, 10]),
    )
    .height(FOOTER_HEIGHT)
    .align_y(Vertical::Center)
    .into()
}

fn hint<'a>(key: &'a str, label: &'a str) -> Element<'a, Message> {
    row![
        container(text(key).size(11).color(palette::TEXT)).padding([1, 6]).style(|_| container::Style {
            background: Some(Background::Color(palette::FAINT)),
            border: Border { radius: 4.0.into(), color: palette::BORDER, width: 1.0 },
            ..Default::default()
        }),
        text(label).size(12).color(palette::DIM),
    ]
    .spacing(6)
    .align_y(Vertical::Center)
    .into()
}

fn action_panel(p: &Panel) -> Element<'_, Message> {
    let input =
        text_input("Search actions…", &p.query).id(actions_id()).on_input(Message::ActionQuery).size(14).padding(8);
    let mut col = column![text(ellipsize_end(&p.title, 32)).size(12).color(palette::DIM), input].spacing(4);
    let actions = p.filtered_actions();
    if actions.is_empty() {
        col = col.push(container(text("No matching actions").size(13).color(palette::DIM)).padding(10));
    }
    for (i, a) in actions.iter().enumerate() {
        let selected = i == p.selected;
        let r = container(
            row![text(a.label()).size(14), space::horizontal(), text(a.shortcut()).size(11).color(palette::DIM)]
                .align_y(Vertical::Center)
                .padding([0, 10]),
        )
        .height(34)
        .width(Length::Fill)
        .align_y(Vertical::Center)
        .style(move |_| container::Style {
            background: selected.then_some(Background::Color(palette::SELECTED)),
            border: Border { radius: 7.0.into(), ..Default::default() },
            ..Default::default()
        });
        col = col.push(mouse_area(r).on_press(Message::PanelClicked(i)).interaction(mouse::Interaction::Pointer));
    }
    col = col.push(hint("Esc", if p.query.is_empty() { "Close actions" } else { "Clear filter" }));
    let panel = container(col).width(280).padding(8).style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb8(0x22, 0x23, 0x29))),
        border: Border { color: palette::BORDER, width: 1.0, radius: 10.0.into() },
        text_color: Some(palette::TEXT),
        ..Default::default()
    });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Right)
        .align_y(Vertical::Bottom)
        .padding([FOOTER_HEIGHT + PAD + 4.0, PAD + 8.0])
        .into()
}

/// `s` cut to `max` characters with a trailing ellipsis.
pub fn ellipsize_end(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `s` cut to `max` characters keeping both ends (paths stay readable).
pub fn ellipsize_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max < 3 {
        return s.to_string();
    }
    let keep = max - 1;
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = s.chars().take(head).collect();
    out.push('…');
    out.extend(s.chars().skip(n - tail));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_shortcuts_work_inside_the_action_panel() {
        let panel = Panel {
            title: "notes.txt".into(),
            actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName],
            query: String::new(),
            selected: 3,
        };
        let key = |key, mods| model::command_for(&key, mods).unwrap();
        assert_eq!(
            panel.action_for_command(key(Key::Named(keyboard::key::Named::Enter), Modifiers::CTRL)),
            Some(Action::Reveal)
        );
        assert_eq!(
            panel.action_for_command(key(Key::Character("C".into()), Modifiers::CTRL | Modifiers::SHIFT)),
            Some(Action::CopyPath)
        );
        assert_eq!(
            panel.action_for_command(key(Key::Named(keyboard::key::Named::Enter), Modifiers::empty())),
            Some(Action::CopyName)
        );
        let app = Panel {
            title: "Files".into(),
            actions: vec![Action::Launch, Action::CopyName],
            query: String::new(),
            selected: 0,
        };
        assert_eq!(app.action_for_command(Command::Secondary), None);
        assert_eq!(app.action_for_command(Command::CopyPath), None);
    }

    #[test]
    fn filtering_actions_keeps_the_target_and_selects_a_matching_action() {
        let mut panel = Panel::new(&Item {
            target: Target::File(FileHit {
                id: "file:///tmp/notes.txt".into(),
                name: "notes.txt".into(),
                path: Some("/tmp/notes.txt".into()),
                icon: String::new(),
                subtext: "/tmp".into(),
                is_dir: false,
            }),
            section: model::Section::Files,
            title: "notes.txt".into(),
            subtitle: "/tmp".into(),
            icon: String::new(),
            tag: "File",
        });
        panel.selected = 3;
        panel.filter("  PATH  copy ".into());
        assert_eq!(panel.title, "notes.txt");
        assert_eq!(panel.selected, 0);
        assert_eq!(panel.filtered_actions(), [Action::CopyPath]);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::CopyPath));
        // Direct shortcuts retain their meaning even when filtered out.
        assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::Reveal));

        panel.filter("does not match".into());
        assert!(panel.filtered_actions().is_empty());
        assert_eq!(panel.action_for_command(Command::Activate), None);
        panel.filter(String::new());
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Open));
        assert_eq!(panel.actions.len(), 4);
    }

    #[test]
    fn ellipsizing() {
        assert_eq!(ellipsize_end("abcdef", 6), "abcdef");
        assert_eq!(ellipsize_end("abcdefg", 6), "abcde…");
        assert_eq!(ellipsize_middle("~/a/very/long/path/name", 11), "~/a/v…/name");
        assert_eq!(ellipsize_middle("short", 11), "short");
        assert_eq!(ellipsize_middle("日本語のパス/ファイル", 7).chars().count(), 7);
    }

    #[test]
    fn heights_fit_the_rows() {
        assert!(full_height() > compact_height());
        assert!(full_height() as f32 >= compact_height() as f32 + ROW_HEIGHT * VISIBLE_ROWS as f32);
    }
}
