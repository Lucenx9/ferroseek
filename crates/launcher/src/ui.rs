// SPDX-License-Identifier: GPL-3.0-or-later
//! The launcher window: an iced program on a wlr-layer-shell overlay.

use std::sync::Mutex;

use iced::alignment::{Horizontal, Vertical};
use iced::event::{self, Event};
use iced::futures::channel::mpsc::UnboundedReceiver;
use iced::futures::stream::{self, BoxStream, StreamExt};
use iced::keyboard::{self, Key, Modifiers};
use iced::mouse::{self, ScrollDelta};
use iced::widget::{
    self, Id, button, column, container, image, mouse_area, row, scrollable, space, stack, svg, text, text_input,
};
use iced::{Background, Border, Color, Element, Length, Shadow, Subscription, Task, Theme, Vector, padding};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};
use iced_layershell::to_layer_message;

use crate::apps::App;
use crate::backend::{ACTION_OPEN_PARENT, Backend, FileHit, RunError, SearchStatus, UiEvent};
use crate::calc;
use crate::favorites::Favorites;
use crate::folders;
use crate::icons::{Icon, Icons};
use crate::model::{self, Action, ClickTracker, Command, Cursor, Item, Scope, Target};
use crate::usage::{self, Kind, Usage};

pub const WIDTH: u32 = 720;
const VISIBLE_ROWS: usize = model::NUMBERED_SLOTS;
const ROW_HEIGHT: f32 = 48.0;
const INPUT_HEIGHT: f32 = 58.0;
const FOOTER_HEIGHT: f32 = 34.0;
const PAD: f32 = 8.0;
const ICON_SIZE: f32 = 28.0;
/// Below this, file search is not run (the daemon needs two characters).
const MIN_FILE_QUERY: usize = 2;
const UNCONFIRMED_OPEN: &str = "Open not confirmed. Check the desktop; close and reopen FerroSeek before trying again.";

fn compact_height() -> u32 {
    (PAD * 2.0 + INPUT_HEIGHT + 1.0 + FOOTER_HEIGHT) as u32
}

fn full_height() -> u32 {
    compact_height() + (ROW_HEIGHT * VISIBLE_ROWS as f32 + PAD) as u32
}

pub(crate) mod palette {
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
    /// Floating layers above the surface, such as the action panel.
    pub const RAISED: Color = Color::from_rgb8(0x22, 0x23, 0x29);
}

static UI_EVENTS: Mutex<Option<UnboundedReceiver<UiEvent>>> = Mutex::new(None);
static BOOT: Mutex<Option<Boot>> = Mutex::new(None);

/// What the launcher starts with.
pub struct Boot {
    pub backend: Backend,
    pub apps: Vec<App>,
    pub usage: Usage,
    pub favorites: Favorites,
    pub initial_query: String,
    pub settings_path: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Idle,
    Searching,
    Error(String),
    Notice(String),
}

#[derive(Debug, Clone, PartialEq)]
enum SearchStatusPanel {
    Loading,
    Ready(SearchStatus),
    Error(String),
}

struct Panel {
    title: String,
    actions: Vec<Action>,
    desktop_labels: Vec<String>,
    query: String,
    cursor: Cursor,
}

impl Panel {
    fn new(item: &Item, favorite: bool) -> Self {
        Self {
            title: item.title.clone(),
            actions: model::actions_for(&item.target, favorite),
            desktop_labels: Vec::new(),
            query: String::new(),
            cursor: Cursor::new(VISIBLE_ROWS),
        }
    }

    fn label(&self, action: Action) -> &str {
        match action {
            Action::Desktop(i) => self.desktop_labels.get(i).map_or(action.label(), String::as_str),
            _ => action.label(),
        }
    }

    fn filtered_actions(&self) -> Vec<Action> {
        let tokens = crate::matcher::tokens(&self.query);
        if tokens.is_empty() {
            return self.actions.clone();
        }
        let mut matches: Vec<_> = self
            .actions
            .iter()
            .copied()
            .filter_map(|action| {
                let score = if matches!(action, Action::Desktop(_)) {
                    let label = self.label(action).to_lowercase();
                    if tokens.iter().map(String::as_str).eq(label.split_whitespace()) {
                        Some(crate::matcher::EXACT)
                    } else {
                        crate::matcher::score(&tokens, &[(label.as_str(), 100)])
                    }
                } else {
                    action.search_score(&tokens)
                };
                score.map(|score| (action, score))
            })
            .collect();
        // Stable ties preserve the familiar order of equally matching actions.
        matches.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
        matches.into_iter().map(|(action, _)| action).collect()
    }

    fn filter(&mut self, query: String) {
        self.query = query;
        self.cursor.reset();
    }

    fn action_for_command(&self, cmd: Command) -> Option<Action> {
        match cmd {
            Command::Activate => self.filtered_actions().get(self.cursor.selected).copied(),
            Command::ActivateVisible(slot) if slot < model::NUMBERED_SLOTS => {
                let actions = self.filtered_actions();
                self.cursor.window(actions.len()).nth(slot).map(|i| actions[i])
            }
            Command::Secondary => model::secondary_action(&self.actions),
            Command::CopyPath => self.actions.contains(&Action::CopyPath).then_some(Action::CopyPath),
            _ => None,
        }
    }
}

pub struct Launcher {
    backend: Backend,
    apps: Vec<App>,
    usage: Usage,
    favorites: Favorites,
    /// The file being opened by the daemon. While set, further opens are
    /// ignored; its use is recorded once the daemon confirms.
    opening: Option<(String, String)>,
    /// A dispatched action lost its confirmation; never duplicate it in this session.
    unconfirmed_open: bool,
    icons: Icons,
    query: String,
    scope: Scope,
    app_hits: Vec<(usize, u32)>,
    /// Original daemon order; history is applied to a copy when rebuilding.
    files: Vec<FileHit>,
    items: Vec<Item>,
    /// Resolved icon per item, filled when the list is rebuilt.
    item_icons: Vec<Option<Icon>>,
    cursor: Cursor,
    clicks: ClickTracker,
    panel: Option<Panel>,
    search_status: Option<SearchStatusPanel>,
    folder_settings: Option<folders::Panel>,
    folder_generation: u64,
    settings_path: std::path::PathBuf,
    deferred_query: Option<String>,
    status_generation: u64,
    generation: u64,
    status: Status,
    compact: bool,
}

#[to_layer_message]
#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    Scope(Scope),
    ActionQuery(String),
    ShowSearchStatus,
    ShowFolders,
    FolderShortcut(folders::Shortcut),
    Folder { generation: u64, event: folders::Event },
    RefreshSearchStatus,
    CloseSearchStatus,
    SearchStatusLoaded { generation: u64, result: Result<SearchStatus, String> },
    Files { generation: u64, result: Result<Option<Vec<FileHit>>, String> },
    Cmd(Command),
    Wheel(f32),
    Selected(Target),
    Clicked(Target),
    ContextMenu(Target),
    PanelClicked { target: Target, action: Action },
    Opened(Result<(), RunError>),
    Ui(UiEvent),
}

fn search_id() -> Id {
    Id::new("search")
}

fn actions_id() -> Id {
    Id::new("action-search")
}

fn status_scroll_id() -> Id {
    Id::new("search-status-scroll")
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
    let Boot { backend, apps, usage, favorites, initial_query, settings_path } =
        BOOT.lock().unwrap_or_else(|e| e.into_inner()).take().expect("boot data set once by run()");
    let first_run = !settings_path.exists();
    let mut state = Launcher {
        backend,
        apps,
        usage,
        favorites,
        opening: None,
        unconfirmed_open: false,
        icons: Icons::new(ICON_SIZE as u16),
        query: String::new(),
        scope: Scope::All,
        app_hits: Vec::new(),
        files: Vec::new(),
        items: Vec::new(),
        item_icons: Vec::new(),
        cursor: Cursor::new(VISIBLE_ROWS),
        clicks: ClickTracker::default(),
        panel: None,
        search_status: None,
        folder_settings: None,
        folder_generation: 0,
        settings_path,
        deferred_query: first_run.then(|| initial_query.clone()),
        status_generation: 0,
        generation: 0,
        status: Status::Idle,
        compact: true,
    };
    let prefill = if first_run {
        state.show_folders(true)
    } else if initial_query.is_empty() {
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
    if mods == Modifiers::CTRL {
        match key.as_ref() {
            Key::Character(",") => return Some(Message::ShowFolders),
            Key::Character("s" | "S") => return Some(Message::FolderShortcut(folders::Shortcut::Save)),
            Key::Character("d" | "D") => return Some(Message::FolderShortcut(folders::Shortcut::Remove)),
            Key::Character("i" | "I") => return Some(Message::ShowSearchStatus),
            Key::Character("r" | "R") => return Some(Message::RefreshSearchStatus),
            _ => {}
        }
    }
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
            model::home_items(
                &self.apps,
                &self.usage.recent(usage::now(), usize::MAX),
                self.favorites.items(),
                self.scope,
                VISIBLE_ROWS,
            )
        } else {
            let mut items: Vec<Item> = if self.scope.shows_calculator(&self.query) {
                model::calc_item(&self.query).into_iter().collect()
            } else {
                Vec::new()
            };
            if !calc::is_explicit(&self.query) {
                let mut files = self.files.clone();
                let now = usage::now();
                model::order_files(&mut files, |f| self.usage.boost(Kind::File, &f.id, &self.query, now));
                items.extend(model::build_items(&self.apps, &self.app_hits, &files));
            }
            items
        };
        let icons = &mut self.icons;
        self.item_icons = self
            .items
            .iter()
            .map(|item| {
                let generic = match item.target {
                    Target::File(ref f) if !f.is_dir => icons.generic_mime_icon(&item.icon),
                    _ => None,
                };
                let file_fallbacks = [generic.as_deref().unwrap_or_default(), "text-x-generic", "unknown"];
                let fallbacks: &[&str] = match item.target {
                    Target::App(_) => &["application-x-executable"],
                    Target::File(ref f) if f.is_dir => &["folder"],
                    Target::File(_) => &file_fallbacks,
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
            && self.search_status.is_none()
            && self.folder_settings.is_none()
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
        self.clicks = ClickTracker::default();
        let Some(item) = self.selected().cloned() else { return Task::none() };
        let opens = matches!(action, Action::Launch | Action::Desktop(_) | Action::Open | Action::Reveal);
        if opens && self.opening.is_some() {
            return Task::none(); // one open at a time; the launcher exits after it
        }
        if opens && self.unconfirmed_open {
            self.status = Status::Error(UNCONFIRMED_OPEN.into());
            return self.resize();
        }
        match (action, &item.target) {
            (Action::Launch | Action::Desktop(_), Target::App(i)) => {
                let app = &self.apps[*i];
                let launched = match action {
                    Action::Desktop(index) => crate::launch::launch_action(app, index),
                    _ => crate::launch::launch(app),
                };
                match launched {
                    Ok(()) => {
                        self.usage.record(Kind::App, &app.id, &self.query, usage::now());
                        iced::exit()
                    }
                    Err(e) => {
                        self.status = Status::Error(format!("Cannot launch {}: {e}", item.title));
                        self.resize()
                    }
                }
            }
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
            (Action::UseResult, Target::Calc(_)) => {
                let Some(query) = calc::continuation(&self.query) else { return Task::none() };
                self.panel = None;
                // Update before moving the cursor: the continued value can
                // be longer than the original expression.
                let change = update(self, Message::Query(query));
                Task::batch([
                    change,
                    widget::operation::focus(search_id()),
                    widget::operation::move_cursor_to_end(search_id()),
                ])
            }
            (Action::AddFavorite | Action::RemoveFavorite, _) => {
                self.set_favorite(&item.target, action == Action::AddFavorite)
            }
            (Action::ResetRanking, _) => self.reset_ranking(&item.target),
            _ => Task::none(),
        }
    }

    fn is_favorite(&self, target: &Target) -> bool {
        match target {
            Target::App(i) => self.apps.get(*i).is_some_and(|app| self.favorites.contains(Kind::App, &app.id)),
            Target::File(file) => self.favorites.contains(Kind::File, &file.id),
            Target::Calc(_) => false,
        }
    }

    fn actions_for(&self, target: &Target) -> Vec<Action> {
        model::actions_for(target, self.is_favorite(target))
    }

    fn panel_for(&self, item: &Item) -> Panel {
        let mut panel = Panel::new(item, self.is_favorite(&item.target));
        if let Target::App(i) = item.target
            && let Some(app) = self.apps.get(i)
        {
            panel.actions.extend((0..app.actions.len()).map(Action::Desktop));
            panel.desktop_labels = app.actions.iter().map(|action| action.name.clone()).collect();
        }
        panel
    }

    fn set_favorite(&mut self, target: &Target, enabled: bool) -> Task<Message> {
        let (kind, key) = match target {
            Target::App(i) => {
                let Some(app) = self.apps.get(*i) else { return Task::none() };
                (Kind::App, app.id.clone())
            }
            Target::File(file) if file.path.is_some() => (Kind::File, file.id.clone()),
            _ => return Task::none(),
        };
        self.status = match self.favorites.set(kind, &key, enabled) {
            Ok(()) => {
                self.panel = None;
                self.rebuild(true);
                let message = if enabled {
                    if self.favorites.is_persistent() {
                        "Added to favorites"
                    } else {
                        "Added to favorites for this session"
                    }
                } else {
                    "Removed from favorites"
                };
                Status::Notice(message.into())
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Status::Error("Favorites are busy. Try again.".into())
            }
            Err(e) => Status::Error(format!("Cannot update favorites: {e}")),
        };
        let focus = if self.panel.is_some() { actions_id() } else { search_id() };
        Task::batch([self.resize(), widget::operation::focus(focus)])
    }

    fn reset_ranking(&mut self, target: &Target) -> Task<Message> {
        let (kind, key) = match target {
            Target::App(i) => (Kind::App, self.apps[*i].id.as_str()),
            Target::File(f) => (Kind::File, f.id.as_str()),
            Target::Calc(_) => return Task::none(),
        };
        self.panel = None;
        self.status = match self.usage.reset(kind, key) {
            Ok(()) => {
                let (usage, now) = (&self.usage, usage::now());
                let q = self.query.as_str();
                self.app_hits = if self.scope.includes_apps() {
                    model::match_apps(&self.apps, q, |a| usage.boost(Kind::App, &a.id, q, now))
                } else {
                    Vec::new()
                };
                self.rebuild(true);
                Status::Notice("Ranking reset".into())
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Status::Error("History is busy. Try reset again.".into())
            }
            Err(e) => Status::Error(format!("Cannot reset ranking: {e}")),
        };
        Task::batch([self.resize(), widget::operation::focus(search_id())])
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
        self.selected().and_then(|i| self.actions_for(&i.target).first().copied())
    }

    fn refresh_search_status(&mut self) -> Task<Message> {
        if matches!(self.search_status, Some(SearchStatusPanel::Loading)) {
            return Task::none();
        }
        self.search_status = Some(SearchStatusPanel::Loading);
        self.status_generation += 1;
        let generation = self.status_generation;
        Task::perform(self.backend.clone().status(), move |result| Message::SearchStatusLoaded { generation, result })
    }

    fn show_folders(&mut self, first_run: bool) -> Task<Message> {
        self.folder_generation += 1;
        let generation = self.folder_generation;
        self.panel = None;
        self.search_status = None;
        self.clicks = ClickTracker::default();
        let mut panel = folders::Panel::new(self.settings_path.clone(), first_run);
        let load = panel.load(self.backend.clone()).map(move |event| Message::Folder { generation, event });
        self.folder_settings = Some(panel);
        Task::batch([load, self.resize()])
    }

    fn finish_folders(&mut self, task: Task<folders::Event>, effect: folders::Effect) -> Task<Message> {
        let generation = self.folder_generation;
        let task = task.map(move |event| Message::Folder { generation, event });
        match effect {
            folders::Effect::None => task,
            folders::Effect::Close { .. } if self.folder_settings.as_ref().is_some_and(|p| p.exit_on_cancel) => {
                iced::exit()
            }
            folders::Effect::Close { refresh } => {
                self.folder_settings = None;
                self.folder_generation += 1;
                let refresh = if refresh { update(self, Message::Query(self.query.clone())) } else { Task::none() };
                Task::batch([task, refresh, self.resize(), widget::operation::focus(search_id())])
            }
            folders::Effect::Applied => {
                self.folder_settings = None;
                self.folder_generation += 1;
                let query = self.deferred_query.take().unwrap_or_else(|| self.query.clone());
                let search = update(self, Message::Query(query));
                self.status = Status::Notice("Folders applied. Indexing continues in the background.".into());
                Task::batch([task, search, self.resize(), widget::operation::focus(search_id())])
            }
        }
    }
}

fn update(s: &mut Launcher, message: Message) -> Task<Message> {
    if s.folder_settings.is_some()
        && matches!(
            message,
            Message::Query(_)
                | Message::Scope(_)
                | Message::ActionQuery(_)
                | Message::Selected(_)
                | Message::Clicked(_)
                | Message::ContextMenu(_)
                | Message::PanelClicked { .. }
                | Message::ShowSearchStatus
        )
    {
        return Task::none();
    }
    // Queued input from the search view must not act through the status view.
    if s.search_status.is_some()
        && matches!(
            message,
            Message::Query(_)
                | Message::Scope(_)
                | Message::ActionQuery(_)
                | Message::Selected(_)
                | Message::Clicked(_)
                | Message::ContextMenu(_)
                | Message::PanelClicked { .. }
        )
    {
        return Task::none();
    }
    match message {
        Message::ShowFolders => {
            if let Some(panel) = &mut s.folder_settings {
                let (task, effect) = panel.update(folders::Event::Back, s.backend.clone());
                s.finish_folders(task, effect)
            } else if s.opening.is_some() {
                Task::none()
            } else {
                s.show_folders(false)
            }
        }
        Message::FolderShortcut(shortcut) => {
            let Some(panel) = &mut s.folder_settings else { return Task::none() };
            let (task, effect) = panel.shortcut(shortcut, s.backend.clone());
            s.finish_folders(task, effect)
        }
        Message::Folder { generation, event } => {
            if generation != s.folder_generation {
                return Task::none();
            }
            let Some(panel) = &mut s.folder_settings else { return Task::none() };
            let (task, effect) = panel.update(event, s.backend.clone());
            s.finish_folders(task, effect)
        }
        Message::RefreshSearchStatus if s.folder_settings.is_some() => {
            let panel = s.folder_settings.as_mut().unwrap();
            let (task, effect) = panel.update(folders::Event::Reload, s.backend.clone());
            s.finish_folders(task, effect)
        }
        Message::ShowSearchStatus => {
            if s.search_status.is_some() {
                return update(s, Message::CloseSearchStatus);
            }
            if s.opening.is_some() {
                return Task::none();
            }
            s.clicks = ClickTracker::default();
            s.panel = None;
            let load = s.refresh_search_status();
            Task::batch([load, s.resize()])
        }
        Message::RefreshSearchStatus if s.search_status.is_some() => s.refresh_search_status(),
        Message::RefreshSearchStatus => Task::none(),
        Message::CloseSearchStatus => {
            s.search_status = None;
            Task::batch([s.resize(), widget::operation::focus(search_id())])
        }
        Message::SearchStatusLoaded { generation, result } => {
            if generation == s.status_generation && s.search_status.is_some() {
                s.search_status = Some(match result {
                    Ok(status) => SearchStatusPanel::Ready(status),
                    Err(error) => SearchStatusPanel::Error(error),
                });
            }
            Task::none()
        }
        Message::Query(q) => {
            s.clicks = ClickTracker::default();
            // Editing the main field leaves the action panel and starts a new search.
            s.panel = None;
            s.query = q;
            s.generation += 1;
            let (usage, now) = (&s.usage, usage::now());
            let q = s.query.as_str();
            // An explicit calculator query stays in that mode while incomplete.
            let calculator = calc::is_explicit(q);
            s.app_hits = if calculator || !s.scope.includes_apps() {
                Vec::new()
            } else {
                model::match_apps(&s.apps, q, |a| usage.boost(Kind::App, &a.id, q, now))
            };
            let file_query = s.scope.file_query(q);
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
        Message::Scope(scope) => {
            s.panel = None;
            let search = if s.scope == scope {
                Task::none()
            } else {
                s.scope = scope;
                let query = s.query.clone();
                update(s, Message::Query(query))
            };
            Task::batch([search, widget::operation::focus(search_id())])
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
                Ok(Some(files)) => {
                    s.files = files;
                    if matches!(s.status, Status::Searching | Status::Idle) {
                        s.status = Status::Idle;
                    }
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
            s.clicks = ClickTracker::default();
            if s.search_status.is_some() || s.folder_settings.is_some() {
                return Task::none(); // the status scrollable handles its own wheel events
            }
            let delta = if dy > 0.0 { -1 } else { 1 };
            if let Some(panel) = &mut s.panel {
                panel.cursor.move_by(delta, panel.filtered_actions().len());
            } else {
                s.cursor.move_by(delta, s.items.len());
            }
            Task::none()
        }
        Message::Selected(target) => {
            if s.panel.is_some() {
                s.clicks = ClickTracker::default();
                s.panel = None;
                return widget::operation::focus(search_id());
            }
            if s.cursor.select_target(&target, &s.items) {
                s.clicks.press(&target);
            } else {
                s.clicks = ClickTracker::default();
            }
            widget::operation::focus(search_id())
        }
        Message::Clicked(target) => {
            if !s.clicks.double_click(&target) || !s.cursor.select_target(&target, &s.items) {
                return Task::none();
            }
            s.panel = None;
            match s.primary() {
                Some(a) => s.perform(a),
                None => Task::none(),
            }
        }
        Message::ContextMenu(target) => {
            s.clicks = ClickTracker::default();
            if !s.cursor.select_target(&target, &s.items) {
                return Task::none();
            }
            s.panel = s.selected().map(|item| s.panel_for(item));
            widget::operation::focus(actions_id())
        }
        Message::PanelClicked { target, action } => {
            // A filter or a late result can change rows after they were drawn.
            // Resolve the action and target identities, never an old row number.
            let a = s
                .selected()
                .filter(|item| item.target == target)
                .and(s.panel.as_ref())
                .filter(|p| p.filtered_actions().contains(&action))
                .map(|_| action);
            a.map_or_else(Task::none, |a| s.perform(a))
        }
        Message::Opened(Ok(())) => {
            if let Some((id, query)) = s.opening.take() {
                s.usage.record(Kind::File, &id, &query, usage::now());
                return iced::exit();
            }
            Task::none()
        }
        Message::Opened(Err(e)) => {
            if s.opening.take().is_none() {
                return Task::none();
            }
            s.status = match e {
                RunError::Failed(message) => Status::Error(format!("Cannot open: {message}")),
                RunError::Unconfirmed(_) => {
                    s.unconfirmed_open = true;
                    Status::Error(UNCONFIRMED_OPEN.into())
                }
            };
            s.resize()
        }
        Message::Ui(UiEvent::Toggle) => iced::exit(),
        // Layer-shell requests generated by `to_layer_message` are handled
        // by the runtime before they reach here.
        _ => Task::none(),
    }
}

fn command(s: &mut Launcher, cmd: Command) -> Task<Message> {
    s.clicks = ClickTracker::default();
    if let Some(panel) = &mut s.folder_settings {
        let (task, effect) = panel.command(cmd, s.backend.clone());
        return s.finish_folders(task, effect);
    }
    if s.search_status.is_some() {
        use widget::operation::{AbsoluteOffset, RelativeOffset};
        return match cmd {
            Command::Back => update(s, Message::CloseSearchStatus),
            Command::Up => widget::operation::scroll_by(status_scroll_id(), AbsoluteOffset { x: 0.0, y: -48.0 }),
            Command::Down => widget::operation::scroll_by(status_scroll_id(), AbsoluteOffset { x: 0.0, y: 48.0 }),
            Command::PageUp => widget::operation::scroll_by(status_scroll_id(), AbsoluteOffset { x: 0.0, y: -240.0 }),
            Command::PageDown => widget::operation::scroll_by(status_scroll_id(), AbsoluteOffset { x: 0.0, y: 240.0 }),
            Command::First => widget::operation::snap_to(status_scroll_id(), RelativeOffset::START),
            Command::Last => widget::operation::snap_to(status_scroll_id(), RelativeOffset::END),
            _ => Task::none(),
        };
    }
    if matches!(cmd, Command::NextScope | Command::PreviousScope) {
        let scope = s.scope.cycle(cmd == Command::NextScope);
        return update(s, Message::Scope(scope));
    }
    let len = s.items.len();
    if let Some(p) = &mut s.panel {
        if let Some(action) = p.action_for_command(cmd) {
            return s.perform(action);
        }
        let action_count = p.filtered_actions().len();
        let page = VISIBLE_ROWS as isize - 1;
        match cmd {
            Command::Up => p.cursor.move_by(-1, action_count),
            Command::Down => p.cursor.move_by(1, action_count),
            Command::PageUp => p.cursor.move_by(-page, action_count),
            Command::PageDown => p.cursor.move_by(page, action_count),
            Command::First => p.cursor.move_by(-(action_count as isize), action_count),
            Command::Last => p.cursor.move_by(action_count as isize, action_count),
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
        Command::ActivateVisible(slot) => {
            if s.cursor.select_visible(slot, len) {
                return s.primary().map_or_else(Task::none, |a| s.perform(a));
            }
        }
        Command::Secondary => {
            let action = s.selected().and_then(|i| model::secondary_action(&s.actions_for(&i.target)));
            if let Some(action) = action {
                return s.perform(action);
            }
        }
        Command::CopyPath => return s.perform(Action::CopyPath),
        Command::NextScope | Command::PreviousScope => unreachable!("handled before the action panel"),
        Command::ToggleActions | Command::OpenActions => {
            if let Some(item) = s.selected() {
                s.panel = Some(s.panel_for(item));
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
    if let Some(panel) = &s.folder_settings {
        let generation = s.folder_generation;
        return panel.view().map(move |event| Message::Folder { generation, event });
    }
    if let Some(status) = &s.search_status {
        return search_status_view(status);
    }
    let placeholder = match s.scope {
        Scope::All => "Search applications and files…",
        Scope::Applications => "Search applications…",
        Scope::Files => "Search indexed files…",
    };
    let input =
        text_input(placeholder, &s.query).id(search_id()).on_input(Message::Query).size(20).padding([14, 8]).style(
            |_, _| text_input::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default(),
                icon: palette::DIM,
                placeholder: palette::DIM,
                value: palette::TEXT,
                selection: palette::SELECTED,
            },
        );
    let scopes = Scope::ALL.into_iter().fold(row![].spacing(3), |choices, scope| {
        let selected = s.scope == scope;
        choices.push(button(text(scope.to_string()).size(11)).on_press(Message::Scope(scope)).padding([6, 8]).style(
            move |_, status| button::Style {
                background: if selected {
                    Some(Background::Color(palette::SELECTED))
                } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    Some(Background::Color(palette::FAINT))
                } else {
                    None
                },
                text_color: if selected { palette::TEXT } else { palette::DIM },
                border: Border { radius: 5.0.into(), ..Default::default() },
                ..Default::default()
            },
        ))
    });
    let index_status = button(text("Index status").size(11))
        .on_press(Message::ShowSearchStatus)
        .padding([6, 8])
        .style(status_button_style);
    let folders =
        button(text("Folders").size(11)).on_press(Message::ShowFolders).padding([6, 8]).style(status_button_style);
    // A hairline separates the result filter from the two screens it does not affect.
    let separator = container(space())
        .width(1)
        .height(16)
        .style(|_| container::Style { background: Some(Background::Color(palette::BORDER)), ..Default::default() });
    let header = container(row![input, scopes, separator, index_status, folders].spacing(4).align_y(Vertical::Center))
        .padding([0, 8])
        .height(INPUT_HEIGHT)
        .align_y(Vertical::Center);

    let mut body = column![header, divider()];
    if !s.compact {
        body =
            body.push(container(results(s)).height(ROW_HEIGHT * VISIBLE_ROWS as f32 + PAD).padding([PAD / 2.0, 0.0]));
        body = body.push(divider());
    }
    body = body.push(footer(s));

    let surface = container(body).padding(PAD).width(Length::Fill).height(Length::Fill).style(surface_style);

    crate::shortcuts::capture_numbered(match (&s.panel, s.selected()) {
        (Some(p), Some(item)) => stack![surface, action_panel(p, &item.target)].into(),
        _ => surface.into(),
    })
}

pub(crate) fn surface_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::SURFACE)),
        border: Border { color: palette::BORDER, width: 1.0, radius: 14.0.into() },
        text_color: Some(palette::TEXT),
        ..Default::default()
    }
}

pub(crate) fn divider<'a, M: 'a>() -> Element<'a, M> {
    container(space::horizontal())
        .height(1)
        .width(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(palette::FAINT)), ..Default::default() })
        .into()
}

fn index_summary(status: &SearchStatus) -> (&'static str, &'static str) {
    match status.state.as_str() {
        "starting" => ("Starting file search", "The search service is preparing the index."),
        "scanning" => ("Indexing files", "Some files may not appear until indexing finishes."),
        "idle" => ("Watching for changes", "Search uses the indexed folders listed below."),
        "paused" => ("Indexing paused", "File changes are not indexed while the service is paused."),
        "stopping" => ("File search is stopping", "Return to search and retry when the service is available."),
        _ => ("Unrecognized search status", "Update the launcher and search service together, then refresh."),
    }
}

/// Editable fields: quiet until focused, then outlined in the accent.
pub(crate) fn field_style(_: &Theme, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(palette::FAINT),
        border: Border {
            color: match status {
                text_input::Status::Focused { .. } => palette::ACCENT,
                _ => palette::BORDER,
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: palette::DIM,
        placeholder: palette::DIM,
        value: palette::TEXT,
        selection: palette::SELECTED,
    }
}

pub(crate) fn status_button_style(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then_some(Background::Color(palette::FAINT)),
        text_color: palette::DIM,
        border: Border { radius: 5.0.into(), ..Default::default() },
        ..Default::default()
    }
}

fn status_metric(value: u64, label: &'static str) -> Element<'static, Message> {
    container(column![text(value.to_string()).size(22), text(label).size(12).color(palette::DIM)].spacing(3))
        .padding(12)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(palette::FAINT)),
            border: Border { radius: 8.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

fn search_status_view(status: &SearchStatusPanel) -> Element<'_, Message> {
    let loading = matches!(status, SearchStatusPanel::Loading);
    let mut refresh = button(hint("Ctrl+R", "Refresh")).padding([6, 8]).style(status_button_style);
    if !loading {
        refresh = refresh.on_press(Message::RefreshSearchStatus);
    }
    let header =
        container(row![text("File search status").size(20), space::horizontal(), refresh].align_y(Vertical::Center))
            .height(INPUT_HEIGHT)
            .padding(padding::left(16).right(PAD))
            .align_y(Vertical::Center);
    let mut content = column![].spacing(12).padding(16);
    match status {
        SearchStatusPanel::Loading => {
            content = content.push(text("Checking file search…").size(18));
            content = content.push(text("Connecting to the search service.").size(13).color(palette::DIM));
        }
        SearchStatusPanel::Error(error) => {
            content = content.push(text("Search status unavailable").size(18).color(palette::ERROR));
            content = content.push(
                text("Refresh to try again. If this persists, update the launcher and search service together.")
                    .size(13),
            );
            content = content.push(text(error).size(12).color(palette::DIM));
        }
        SearchStatusPanel::Ready(info) => {
            let (title, help) = index_summary(info);
            content = content.push(text(title).size(18));
            content = content.push(text(help).size(13).color(palette::DIM));
            content = content.push(
                row![
                    status_metric(info.files, "Indexed files"),
                    status_metric(info.dirs, "Indexed folders"),
                    status_metric(info.queued_dirs, "Queued folders")
                ]
                .spacing(8),
            );
            if info.watch_limit_hit {
                content = content.push(
                    text("Folder watch limit reached. Some changes may not be detected promptly.")
                        .size(13)
                        .color(palette::ERROR),
                );
            }
            if info.errors > 0 {
                content = content.push(text(format!("{} indexing errors reported during this service session. Check folder access and the service log for details.", info.errors)).size(13).color(palette::ERROR));
            }
            content = content.push(text("Indexed locations").size(14));
            if info.roots.is_empty() {
                content = content.push(text("No indexed locations yet. Check the indexing configuration; locations may also be unavailable while the first scan starts.").size(13).color(palette::DIM));
            } else {
                for root in &info.roots {
                    content = content.push(text(root).size(13).wrapping(text::Wrapping::WordOrGlyph));
                }
            }
        }
    }
    let detail = scrollable(content).id(status_scroll_id()).height(Length::Fill);
    let footer = container(
        row![
            text("Snapshot · Refresh to check for changes").size(12).color(palette::DIM),
            space::horizontal(),
            button(hint("Esc", "Back to search"))
                .on_press(Message::CloseSearchStatus)
                .padding([4, 8])
                .style(status_button_style),
        ]
        .align_y(Vertical::Center),
    )
    .height(FOOTER_HEIGHT)
    .padding(padding::left(16).right(PAD))
    .align_y(Vertical::Center);
    container(column![header, divider(), detail, divider(), footer])
        .padding(PAD)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(surface_style)
        .into()
}

fn results(s: &Launcher) -> Element<'_, Message> {
    if s.items.is_empty() {
        let (msg, help) = match &s.status {
            Status::Searching => ("Searching…", ""),
            Status::Error(_) if s.scope == Scope::Files => {
                ("File search unavailable", "Switch to Applications to search installed apps.")
            }
            Status::Error(_) => ("File search unavailable", "You can still search applications or calculate a result."),
            _ if calc::is_explicit(&s.query) => ("Calculator", "Enter a valid expression, for example = (2 + 3) * 4."),
            _ if s.scope == Scope::Files && s.query.trim().chars().count() < MIN_FILE_QUERY => {
                ("Search indexed files", "Type at least 2 characters to search files.")
            }
            _ if s.scope == Scope::Applications => ("No matching applications", "Try a different application name."),
            _ if s.query.trim().is_empty() => ("Start typing to search", "Find an application, a file, or try 2 + 3."),
            _ if s.query.trim().chars().count() < MIN_FILE_QUERY => {
                ("No matching applications", "Type at least 2 characters to search files.")
            }
            _ => ("No results", "Try a shorter name or a different keyword."),
        };
        let mut empty =
            column![text(msg).size(18), text(help).size(13).color(palette::DIM)].spacing(8).align_x(Horizontal::Center);
        if s.scope.file_query(&s.query) && !matches!(s.status, Status::Searching) {
            empty = empty.push(
                row![
                    button(text("Check file search status").size(13))
                        .on_press(Message::ShowSearchStatus)
                        .style(status_button_style),
                    button(text("Choose folders").size(13)).on_press(Message::ShowFolders).style(status_button_style),
                ]
                .spacing(8),
            );
        }
        return container(empty).center(Length::Fill).into();
    }
    let mut col = column![];
    for (slot, i) in s.cursor.window(s.items.len()).enumerate() {
        col = col.push(result_row(s, i, slot));
    }
    col.into()
}

fn result_row(s: &Launcher, i: usize, slot: usize) -> Element<'_, Message> {
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
    let mut content = row![icon, container(labels).width(Length::Fill).clip(true)];
    if s.is_favorite(&item.target) {
        content = content.push(text("★").size(13).color(palette::ACCENT));
    }
    let mut content = content.push(tag);
    if s.panel.is_none() {
        content = content.push(numbered_hint(slot));
    }
    let content = content.spacing(12).align_y(Vertical::Center).padding(padding::left(9).right(16));
    // A capsule inset from the row's rounded edge; 4 + 3 + 9 px keeps the icon on the 24 px gutter.
    let bar = container(space::vertical()).width(3).height(ROW_HEIGHT / 2.0).style(move |_| container::Style {
        background: selected.then_some(Background::Color(palette::ACCENT)),
        border: Border { radius: 1.5.into(), ..Default::default() },
        ..Default::default()
    });
    let bar = container(bar).padding(padding::left(4));
    let row = container(row![bar, content].align_y(Vertical::Center))
        .height(ROW_HEIGHT)
        .width(Length::Fill)
        .align_y(Vertical::Center)
        .style(move |_| container::Style {
            background: selected.then_some(Background::Color(palette::SELECTED)),
            border: Border { radius: 9.0.into(), ..Default::default() },
            ..Default::default()
        });
    mouse_area(row)
        .on_press(Message::Selected(item.target.clone()))
        .on_double_click(Message::Clicked(item.target.clone()))
        .on_right_press(Message::ContextMenu(item.target.clone()))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn footer(s: &Launcher) -> Element<'_, Message> {
    if s.unconfirmed_open {
        return container(text(UNCONFIRMED_OPEN).size(12).color(palette::ERROR))
            .width(Length::Fill)
            .height(FOOTER_HEIGHT)
            .padding([0, 16])
            .align_y(Vertical::Center)
            .into();
    }
    let (msg, color) = match &s.status {
        Status::Error(e) => (ellipsize_end(e, 70), palette::ERROR),
        Status::Notice(n) => (n.clone(), palette::ACCENT),
        Status::Searching => ("Searching…".to_string(), palette::DIM),
        Status::Idle if calc::is_explicit(&s.query) => ("Calculator".to_string(), palette::DIM),
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
        let acts = s.actions_for(&item.target);
        hints = hints.push(hint("Enter", acts[0].label()));
        if let Some(action) = model::secondary_action(&acts) {
            hints = hints.push(hint("Ctrl+Enter", action.label()));
        }
        hints = hints.push(
            button(hint("Ctrl+K", "Actions")).on_press(Message::Cmd(Command::ToggleActions)).padding([3, 6]).style(
                |_, status| button::Style {
                    background: matches!(status, button::Status::Hovered | button::Status::Pressed)
                        .then_some(Background::Color(palette::FAINT)),
                    text_color: palette::TEXT,
                    ..Default::default()
                },
            ),
        );
    } else {
        // Same inset as the Actions button, so the last hint keeps the right edge.
        hints = hints
            .push(container(hint("Esc", if s.query.is_empty() { "Close" } else { "Clear search" })).padding([3, 6]));
    }
    container(
        row![text(msg).size(12).color(color).wrapping(text::Wrapping::None), space::horizontal(), hints]
            .align_y(Vertical::Center)
            .padding(padding::left(16).right(10)),
    )
    .height(FOOTER_HEIGHT)
    .align_y(Vertical::Center)
    .into()
}

pub(crate) fn hint<'a, M: 'a>(key: &'a str, label: &'a str) -> Element<'a, M> {
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

fn numbered_hint(slot: usize) -> Element<'static, Message> {
    let label = if slot < model::NUMBERED_SLOTS { format!("Ctrl+{}", slot + 1) } else { String::new() };
    text(label).size(11).color(palette::DIM).into()
}

fn action_panel<'a>(p: &'a Panel, target: &Target) -> Element<'a, Message> {
    let input = text_input("Search actions…", &p.query)
        .id(actions_id())
        .on_input(Message::ActionQuery)
        .size(14)
        .padding([8, 10])
        .style(field_style);
    // Title, field, rows and footer share the rows' 10 px text inset.
    let title = container(text(ellipsize_end(&p.title, 32)).size(12).color(palette::DIM)).padding([0, 10]);
    let mut col = column![title, input].spacing(4);
    let actions = p.filtered_actions();
    if actions.is_empty() {
        col = col.push(container(text("No matching actions").size(13).color(palette::DIM)).padding(10));
    }
    for (slot, i) in p.cursor.window(actions.len()).enumerate() {
        let a = actions[i];
        let selected = i == p.cursor.selected;
        let r = container(
            row![
                text(ellipsize_end(p.label(a), 32)).size(14),
                space::horizontal(),
                text(a.shortcut()).size(11).color(palette::DIM),
                numbered_hint(slot),
            ]
            .spacing(10)
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
        col = col.push(
            mouse_area(r)
                .on_press(Message::PanelClicked { target: target.clone(), action: a })
                .interaction(mouse::Interaction::Pointer),
        );
    }
    let mut footer = row![hint("Esc", if p.query.is_empty() { "Close actions" } else { "Clear filter" })]
        .align_y(Vertical::Center)
        .padding([4, 10]);
    if actions.len() > VISIBLE_ROWS {
        let window = p.cursor.window(actions.len());
        footer = footer.push(space::horizontal()).push(
            text(format!("{}–{} / {}", window.start + 1, window.end, actions.len())).size(11).color(palette::DIM),
        );
    }
    col = col.push(footer);
    let panel = container(col).width(360).padding(8).style(|_| container::Style {
        background: Some(Background::Color(palette::RAISED)),
        border: Border { color: palette::BORDER, width: 1.0, radius: 10.0.into() },
        text_color: Some(palette::TEXT),
        // Lifts the panel off the identical results behind it.
        shadow: Shadow { color: Color::from_rgba8(0, 0, 0, 0.45), offset: Vector::new(0.0, 6.0), blur_radius: 18.0 },
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

    fn search_state() -> Launcher {
        let app = crate::apps::parse(
            "report.desktop",
            std::path::Path::new("/synthetic"),
            "[Desktop Entry]\nType=Application\nName=Report Viewer\nExec=viewer\n",
            &[],
            &[],
        )
        .unwrap();
        Launcher {
            backend: Backend::disconnected(),
            apps: vec![app],
            usage: Usage::disabled(),
            favorites: Favorites::disabled(),
            opening: None,
            unconfirmed_open: false,
            icons: Icons::disabled(),
            query: String::new(),
            scope: Scope::All,
            app_hits: Vec::new(),
            files: Vec::new(),
            items: Vec::new(),
            item_icons: Vec::new(),
            cursor: Cursor::new(VISIBLE_ROWS),
            clicks: ClickTracker::default(),
            panel: None,
            search_status: None,
            folder_settings: None,
            folder_generation: 0,
            settings_path: "/synthetic/config.toml".into(),
            deferred_query: None,
            status_generation: 0,
            generation: 0,
            status: Status::Idle,
            compact: true,
        }
    }

    fn file_hits() -> Vec<FileHit> {
        ["report.txt", "report.pdf"]
            .into_iter()
            .map(|name| FileHit {
                id: format!("file:///synthetic/{name}"),
                name: name.into(),
                path: Some(std::path::Path::new("/synthetic").join(name)),
                icon: String::new(),
                subtext: "/synthetic".into(),
                is_dir: false,
            })
            .collect()
    }

    #[test]
    fn scope_changes_replace_results_and_reject_late_replies() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let old_generation = s.generation;
        let _ = update(&mut s, Message::Files { generation: old_generation, result: Ok(Some(file_hits())) });
        assert_eq!(s.items.len(), 3);
        s.cursor.select(2, s.items.len());
        let _ = command(&mut s, Command::OpenActions);
        assert!(s.panel.is_some());

        let _ = command(&mut s, Command::NextScope);
        assert_eq!(s.scope, Scope::Applications);
        assert_eq!(s.query, "report");
        assert_eq!(s.generation, old_generation + 1);
        assert_eq!(s.status, Status::Idle);
        assert!(s.panel.is_none());
        assert!(s.files.is_empty());
        assert_eq!(s.cursor.selected, 0);
        assert_eq!(s.items.len(), 1);
        assert!(matches!(s.items[0].target, Target::App(_)));
        let _ = update(&mut s, Message::Files { generation: old_generation, result: Ok(Some(file_hits())) });
        let _ = update(&mut s, Message::Files { generation: old_generation, result: Err("old error".into()) });
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.status, Status::Idle);

        let _ = command(&mut s, Command::NextScope);
        assert_eq!(s.scope, Scope::Files);
        assert_eq!(s.query, "report");
        assert_eq!(s.status, Status::Searching);
        assert!(s.app_hits.is_empty());
        assert!(s.items.is_empty());
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        assert_eq!(s.items.len(), 2);
        assert!(s.items.iter().all(|i| matches!(i.target, Target::File(_))));
        let target = s.items[0].target.clone();
        let _ = s.reset_ranking(&target);
        assert!(s.app_hits.is_empty());
        assert_eq!(s.items.len(), 2);
        let _ = command(&mut s, Command::ActivateVisible(1));
        assert_eq!(s.opening.as_ref().unwrap().0, "file:///synthetic/report.pdf");
    }

    #[test]
    fn explicit_calculator_overrides_scope_but_implicit_calculations_obey_it() {
        let mut s = search_state();
        for scope in Scope::ALL {
            let _ = update(&mut s, Message::Scope(scope));
            let _ = update(&mut s, Message::Query("= 40 + 2".into()));
            assert_eq!(s.items.len(), 1);
            assert_eq!(s.items[0].target, Target::Calc("42".into()));
            assert_eq!(s.status, Status::Idle);
            assert!(s.app_hits.is_empty());
            let _ = update(&mut s, Message::Query("= (".into()));
            assert!(s.items.is_empty());
            assert_eq!(s.status, Status::Idle);
        }
        for scope in [Scope::Applications, Scope::Files] {
            let _ = update(&mut s, Message::Scope(scope));
            let _ = update(&mut s, Message::Query("40 + 2".into()));
            assert!(s.items.is_empty());
            assert_eq!(s.status, if scope == Scope::Files { Status::Searching } else { Status::Idle });
        }
    }

    #[test]
    fn editing_main_query_leaves_actions_while_action_filter_keeps_the_query() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let _ = command(&mut s, Command::OpenActions);
        let generation = s.generation;
        let _ = update(&mut s, Message::ActionQuery("copy".into()));
        assert_eq!(s.query, "report");
        assert_eq!(s.generation, generation);
        assert_eq!(s.panel.as_ref().unwrap().query, "copy");
        let _ = update(&mut s, Message::Query("report2".into()));
        assert_eq!(s.query, "report2");
        assert_eq!(s.generation, generation + 1);
        assert!(s.panel.is_none());
    }

    #[test]
    fn session_favorites_toggle_from_actions_without_changing_ranking() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let target = s.selected().unwrap().target.clone();
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("pin".into()));
        assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::AddFavorite]);
        let _ = command(&mut s, Command::ActivateVisible(0));
        assert!(s.is_favorite(&target));
        assert_eq!(s.status, Status::Notice("Added to favorites for this session".into()));
        assert_eq!(s.query, "report");
        assert!(s.panel.is_none());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
        assert_eq!(s.usage.boost(Kind::App, "report.desktop", "report", usage::now()), 0);
        let _ = s.reset_ranking(&target);
        assert!(s.is_favorite(&target), "reset ranking must not remove an explicit favorite");
        let _ = update(&mut s, Message::Query(String::new()));
        assert_eq!(s.items.len(), 1, "no duplicate app fallback");
        assert_eq!(s.items[0].target, target);
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("unpin".into()));
        assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::RemoveFavorite]);
        let _ = command(&mut s, Command::Activate);
        assert!(!s.is_favorite(&target));
        assert_eq!(s.items.len(), 1, "unpinning retains the available application suggestion");
    }

    #[test]
    fn favorite_files_lead_home_suggestions_and_respect_scope_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.txt");
        std::fs::write(&path, "synthetic").unwrap();
        let id = format!("file://{}", path.display());
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let hit = FileHit {
            id: id.clone(),
            path: Some(path),
            name: "report.txt".into(),
            icon: String::new(),
            subtext: String::new(),
            is_dir: false,
        };
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![hit])) });
        s.cursor.select(1, s.items.len());
        let target = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::AddFavorite);
        assert!(s.is_favorite(&target));
        let _ = update(&mut s, Message::Query(String::new()));
        assert!(matches!(&s.items[0].target, Target::File(file) if file.id == id));
        assert_eq!(s.items.len(), 2, "favorite before the app fallback");
        let _ = update(&mut s, Message::Scope(Scope::Applications));
        assert!(s.items.iter().all(|item| matches!(item.target, Target::App(_))));
        assert!(s.is_favorite(&target));
        let _ = update(&mut s, Message::Scope(Scope::Files));
        assert_eq!(s.items.len(), 1);
        assert!(matches!(&s.items[0].target, Target::File(file) if file.id == id));
        let _ = s.perform(Action::RemoveFavorite);
        assert!(s.items.is_empty(), "no history or app fallback in Files mode");
        assert!(s.panel.is_none());
    }

    #[test]
    fn busy_favorite_store_preserves_the_menu_and_allows_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-favorites");
        let mut s = search_state();
        s.favorites = Favorites::load(Some(path.clone()));
        let _ = update(&mut s, Message::Query("report".into()));
        let target = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::AddFavorite);
        assert_eq!(s.status, Status::Notice("Added to favorites".into()));
        let lock = std::fs::OpenOptions::new().read(true).write(true).open(path.with_extension("lock")).unwrap();
        lock.lock().unwrap();
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("unpin".into()));
        let _ = command(&mut s, Command::Activate);
        assert!(s.is_favorite(&target));
        assert!(s.panel.is_some());
        assert_eq!(s.status, Status::Error("Favorites are busy. Try again.".into()));
        drop(lock);
        let _ = command(&mut s, Command::Activate);
        assert!(!s.is_favorite(&target));
        assert!(s.panel.is_none());
    }

    #[test]
    fn numbered_shortcuts_are_not_dispatched_again_by_the_subscription() {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Character("1".into()),
            modified_key: Key::Character("1".into()),
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Digit1),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::CTRL,
            text: Some("1".into()),
            repeat: false,
        });
        assert!(on_event(event, event::Status::Captured, iced::window::Id::unique()).is_none());
    }

    #[test]
    fn displayed_shortcuts_work_inside_the_action_panel() {
        let panel = Panel {
            title: "notes.txt".into(),
            actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName],
            query: String::new(),
            desktop_labels: Vec::new(),
            cursor: Cursor { selected: 3, ..Cursor::new(VISIBLE_ROWS) },
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
            desktop_labels: Vec::new(),
            cursor: Cursor { selected: 0, ..Cursor::new(VISIBLE_ROWS) },
        };
        assert_eq!(app.action_for_command(Command::Secondary), None);
        assert_eq!(app.action_for_command(Command::CopyPath), None);
    }

    #[test]
    fn filtering_actions_keeps_the_target_and_selects_a_matching_action() {
        let mut panel = Panel::new(
            &Item {
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
            },
            false,
        );
        panel.cursor.selected = 3;
        panel.filter("  PATH  copy ".into());
        assert_eq!(panel.title, "notes.txt");
        assert_eq!(panel.cursor.selected, 0);
        assert_eq!(panel.filtered_actions(), [Action::CopyPath]);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::CopyPath));
        // Direct shortcuts retain their meaning even when filtered out.
        assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::Reveal));

        panel.filter("RESET rank".into());
        assert_eq!(panel.filtered_actions(), [Action::ResetRanking]);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::ResetRanking));

        panel.filter("does not match".into());
        assert!(panel.filtered_actions().is_empty());
        assert_eq!(panel.action_for_command(Command::Activate), None);
        panel.filter(String::new());
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Open));
        assert_eq!(panel.actions.len(), 6);
    }

    #[test]
    fn numbered_actions_follow_the_filter_without_falling_back_to_the_selection() {
        let mut panel = Panel {
            title: "notes.txt".into(),
            actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName, Action::ResetRanking],
            query: String::new(),
            desktop_labels: Vec::new(),
            cursor: Cursor { selected: 4, ..Cursor::new(VISIBLE_ROWS) },
        };
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Open));
        panel.filter("copy".into());
        panel.cursor.selected = 1;
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::CopyPath));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), Some(Action::CopyName));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(2)), None);
        assert_eq!(panel.action_for_command(Command::ActivateVisible(8)), None);
        assert_eq!(panel.action_for_command(Command::ActivateVisible(usize::MAX)), None);
        assert_eq!(panel.cursor.selected, 1);
        assert_eq!(panel.query, "copy");
        panel.filter("missing".into());
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), None);
    }

    #[test]
    fn action_search_ranks_precise_matches_and_keeps_activation_consistent() {
        let mut panel = Panel {
            title: "notes.txt".into(),
            actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName, Action::ResetRanking],
            query: String::new(),
            desktop_labels: Vec::new(),
            cursor: Cursor { selected: 0, ..Cursor::new(VISIBLE_ROWS) },
        };
        let original = panel.actions.clone();
        for query in ["", "  "] {
            panel.filter(query.into());
            assert_eq!(panel.filtered_actions(), original);
        }
        for (query, action) in [
            ("copnm", Action::CopyName),
            ("COPY filename", Action::CopyName),
            ("reveal", Action::Reveal),
            ("directory", Action::Reveal),
            ("copy location", Action::CopyPath),
            ("rank reset", Action::ResetRanking),
        ] {
            panel.filter(query.into());
            assert_eq!(panel.filtered_actions(), [action], "{query}");
            assert_eq!(panel.action_for_command(Command::Activate), Some(action));
            assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(action));
            assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), None);
        }
        panel.filter("name".into());
        assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::Reveal));
        assert_eq!(panel.action_for_command(Command::CopyPath), Some(Action::CopyPath));
        panel.filter("open".into());
        assert_eq!(panel.filtered_actions().first(), Some(&Action::Open));
        panel.filter("r".into());
        assert_eq!(panel.filtered_actions(), [Action::ResetRanking, Action::Reveal]);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::ResetRanking));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::ResetRanking));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), Some(Action::Reveal));
        panel.filter("copy".into());
        assert_eq!(panel.filtered_actions(), [Action::CopyPath, Action::CopyName], "ties keep their order");
        panel.filter("copy xyz".into());
        assert!(panel.filtered_actions().is_empty());
        assert_eq!(panel.action_for_command(Command::Activate), None);
    }

    #[test]
    fn action_clicks_keep_their_identity_after_filtering_and_ignore_changed_targets() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        s.cursor.select(1, s.items.len());
        let target = s.selected().unwrap().target.clone();
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("copy".into()));
        let status = s.status.clone();
        let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: Action::Open });
        assert!(s.opening.is_none(), "a removed action must not run the row that replaced it");
        assert_eq!(s.status, status);
        assert_eq!(s.panel.as_ref().unwrap().query, "copy");

        let _ = update(&mut s, Message::ActionQuery("name".into()));
        let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: Action::CopyName });
        assert_eq!(s.status, Status::Notice("Copied name".into()));
        assert!(s.panel.is_none());
        assert_eq!(s.query, "report");

        let _ = update(&mut s, Message::Query("report".into()));
        let _ = command(&mut s, Command::OpenActions);
        let status = s.status.clone();
        let _ = update(&mut s, Message::PanelClicked { target, action: Action::CopyName });
        assert_eq!(s.status, status, "an old file click must not copy the newly selected app name");
        assert!(s.panel.is_some());
    }

    #[test]
    fn calculator_secondary_action_is_available_while_filtering_for_copy() {
        let mut panel = Panel::new(&model::calc_item("= 1/3").unwrap(), false);
        panel.filter("copy".into());
        assert_eq!(panel.filtered_actions(), [Action::CopyResult]);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::CopyResult));
        assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::UseResult));
        panel.filter("use".into());
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::UseResult));
        assert_eq!(Action::UseResult.shortcut(), "Ctrl+Enter");
    }

    fn search_info(state: &str) -> SearchStatus {
        SearchStatus {
            state: state.into(),
            files: 42,
            dirs: 7,
            queued_dirs: 3,
            watch_limit_hit: false,
            errors: 0,
            roots: vec!["/synthetic/Projects:2026".into()],
        }
    }

    #[test]
    fn search_status_refresh_ignores_old_replies_and_preserves_the_search() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let query_generation = s.generation;
        let selected = s.selected().unwrap().target.clone();
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert!(s.panel.is_none());
        assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
        let first = s.status_generation;
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.status_generation, first, "only one status request may be in flight");
        let _ = update(&mut s, Message::SearchStatusLoaded { generation: first, result: Ok(search_info("scanning")) });
        assert_eq!(s.search_status, Some(SearchStatusPanel::Ready(search_info("scanning"))));
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let second = s.status_generation;
        assert!(second > first);
        let _ = update(&mut s, Message::SearchStatusLoaded { generation: first, result: Err("old failure".into()) });
        assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
        let _ = command(&mut s, Command::Back);
        assert!(s.search_status.is_none());
        let _ = update(&mut s, Message::SearchStatusLoaded { generation: second, result: Ok(search_info("idle")) });
        assert!(s.search_status.is_none());
        let _ = update(&mut s, Message::ShowSearchStatus);
        let third = s.status_generation;
        let _ = update(&mut s, Message::SearchStatusLoaded { generation: second, result: Ok(search_info("paused")) });
        assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
        let _ = update(&mut s, Message::SearchStatusLoaded { generation: third, result: Ok(search_info("idle")) });
        let _ = update(&mut s, Message::CloseSearchStatus);
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::All);
        assert_eq!(s.generation, query_generation);
        assert_eq!(s.selected().unwrap().target, selected);
    }

    #[test]
    fn search_status_blocks_queued_result_actions_and_scope_navigation() {
        let mut s = search_state();
        // A misrouted activation must never spawn an external program in this test.
        s.apps[0].exec.clear();
        let _ = update(&mut s, Message::Query("report".into()));
        let target = s.selected().unwrap().target.clone();
        let generation = s.generation;
        let search_status = s.status.clone();
        let _ = update(&mut s, Message::ShowSearchStatus);
        for message in [
            Message::Query("changed".into()),
            Message::Scope(Scope::Files),
            Message::Selected(target.clone()),
            Message::Clicked(target.clone()),
            Message::ContextMenu(target.clone()),
            Message::PanelClicked { target, action: Action::Launch },
            Message::Wheel(-1.0),
        ] {
            let _ = update(&mut s, message);
        }
        for cmd in [
            Command::Activate,
            Command::ActivateVisible(0),
            Command::Secondary,
            Command::NextScope,
            Command::PreviousScope,
            Command::CopyPath,
            Command::OpenActions,
        ] {
            let _ = command(&mut s, cmd);
        }
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::All);
        assert_eq!(s.cursor.selected, 0);
        assert!(s.panel.is_none());
        assert!(s.opening.is_none());
        assert_eq!(s.status, search_status);
        // A current file reply can finish without replacing the status view.
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
        assert_eq!(s.files.len(), 2);
    }

    #[test]
    fn folder_settings_are_modal_and_discard_late_replies_after_reopening() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let search_generation = s.generation;
        let _ = update(&mut s, Message::ShowFolders);
        let old = s.folder_generation;
        assert!(s.folder_settings.is_some());
        assert!(!s.compact);
        for message in [Message::Query("changed".into()), Message::Scope(Scope::Files), Message::ShowSearchStatus] {
            let _ = update(&mut s, message);
        }
        let _ = command(&mut s, Command::ActivateVisible(0));
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::All);
        assert_eq!(s.generation, search_generation);
        assert!(s.opening.is_none());
        let _ = update(&mut s, Message::Files { generation: search_generation, result: Ok(Some(file_hits())) });
        assert!(s.folder_settings.is_some());
        assert_eq!(s.files.len(), 2);
        let _ = command(&mut s, Command::Back);
        assert!(s.folder_settings.is_none());
        assert_eq!(s.query, "report");
        let _ = update(&mut s, Message::ShowFolders);
        let _ = update(&mut s, Message::Folder { generation: old, event: folders::Event::Back });
        assert!(s.folder_settings.is_some(), "a late event cannot close a reopened settings view");
    }

    #[test]
    fn first_run_defers_file_queries_until_folders_are_applied() {
        let mut s = search_state();
        s.deferred_query = Some("report".into());
        let _ = s.show_folders(true);
        let _ = update(&mut s, Message::Query("report".into()));
        assert_eq!(s.generation, 0);
        assert!(s.query.is_empty());
        let _ = s.finish_folders(Task::none(), folders::Effect::Applied);
        assert!(s.folder_settings.is_none());
        assert!(s.deferred_query.is_none());
        assert_eq!(s.query, "report");
        assert_eq!(s.generation, 1);
        assert!(s.files.is_empty());
        assert!(matches!(key_message(&Key::Character(",".into()), Modifiers::CTRL), Some(Message::ShowFolders)));
    }

    #[test]
    fn search_status_errors_can_retry_and_empty_search_restores_its_size() {
        let mut s = search_state();
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.status_generation, 0, "refresh outside the view must not activate the daemon");
        s.opening = Some(("file:///synthetic/report.txt".into(), String::new()));
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert!(s.search_status.is_none(), "an ongoing file open must finish before opening status");
        assert_eq!(s.status_generation, 0);
        s.opening = None;
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert!(!s.compact);
        let generation = s.status_generation;
        let _ = update(&mut s, Message::SearchStatusLoaded { generation, result: Err("service unavailable".into()) });
        assert_eq!(s.search_status, Some(SearchStatusPanel::Error("service unavailable".into())));
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert!(s.search_status.is_none(), "the status shortcut also closes the view");
        assert!(s.compact);
        assert_eq!(
            key_message(&Key::Character("I".into()), Modifiers::CTRL).map(|m| matches!(m, Message::ShowSearchStatus)),
            Some(true)
        );
        assert_eq!(
            key_message(&Key::Character("r".into()), Modifiers::CTRL)
                .map(|m| matches!(m, Message::RefreshSearchStatus)),
            Some(true)
        );
    }

    #[test]
    fn search_status_describes_paused_and_unknown_states_without_claiming_readiness() {
        assert_eq!(index_summary(&search_info("scanning")).0, "Indexing files");
        assert_eq!(index_summary(&search_info("paused")).0, "Indexing paused");
        assert_eq!(index_summary(&search_info("idle")).0, "Watching for changes");
        assert_eq!(index_summary(&search_info("future-state")).0, "Unrecognized search status");
    }

    #[test]
    fn unconfirmed_open_blocks_duplicates_but_preserves_search_and_copy() {
        let mut s = search_state();
        s.apps[0].exec.clear(); // a regression must not spawn a program in this test
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        s.cursor.select(1, s.items.len());
        let _ = command(&mut s, Command::Activate);
        let (id, query) = s.opening.clone().unwrap();
        let _ = update(&mut s, Message::Opened(Err(RunError::Unconfirmed("timed out".into()))));
        assert!(s.unconfirmed_open);
        assert!(s.opening.is_none());
        assert_eq!(s.usage.boost(Kind::File, &id, &query, usage::now()), 0);
        for cmd in [Command::Activate, Command::Secondary, Command::ActivateVisible(2)] {
            let _ = command(&mut s, cmd);
            assert!(s.opening.is_none());
            assert_eq!(s.status, Status::Error(UNCONFIRMED_OPEN.into()));
        }
        let _ = update(&mut s, Message::Opened(Ok(())));
        assert_eq!(
            s.usage.boost(Kind::File, &id, &query, usage::now()),
            0,
            "late confirmation must not record use or exit"
        );
        let _ = update(&mut s, Message::Query("report".into()));
        let _ = command(&mut s, Command::Activate);
        assert_eq!(s.status, Status::Error(UNCONFIRMED_OPEN.into()), "app launches are blocked too");
        let _ = s.perform(Action::CopyName);
        assert_eq!(s.status, Status::Notice("Copied name".into()));
        assert!(s.unconfirmed_open, "copying must not unlock another open");
        let _ = update(&mut s, Message::Query("= 2+3".into()));
        let _ = command(&mut s, Command::Activate);
        assert_eq!(s.status, Status::Notice("Copied result".into()));
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
        let _ = command(&mut s, Command::Back);
        assert!(s.unconfirmed_open);
    }

    #[test]
    fn rejected_open_can_be_corrected_and_only_success_records_usage() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        s.cursor.select(1, s.items.len());
        let _ = command(&mut s, Command::Activate);
        let (id, query) = s.opening.clone().unwrap();
        let _ = update(&mut s, Message::Opened(Err(RunError::Failed("path is not indexed".into()))));
        assert!(!s.unconfirmed_open);
        assert!(s.opening.is_none());
        assert_eq!(s.usage.boost(Kind::File, &id, &query, usage::now()), 0);
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_some(), "a rejected request permits an explicit corrected attempt");
        let _ = update(&mut s, Message::Opened(Ok(())));
        assert!(s.opening.is_none());
        assert!(s.usage.boost(Kind::File, &id, &query, usage::now()) > 0);
        let now = usage::now();
        let before = s.usage.boost(Kind::File, &id, &query, now);
        let _ = update(&mut s, Message::Opened(Ok(())));
        assert_eq!(s.usage.boost(Kind::File, &id, &query, now), before);
    }

    #[test]
    fn stale_search_failure_does_not_replace_a_newer_success() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("old report".into()));
        let old = s.generation;
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let items = s.items.clone();
        let _ = update(&mut s, Message::Files { generation: old, result: Err("timed out".into()) });
        assert_eq!(s.status, Status::Idle);
        assert_eq!(s.items, items);
    }

    fn application_actions(names: &[&str]) -> Launcher {
        let mut s = search_state();
        s.apps[0].actions = names
            .iter()
            .enumerate()
            .map(|(i, name)| crate::apps::DesktopAction {
                id: format!("action-{i}"),
                name: (*name).into(),
                exec: format!("synthetic-viewer --action-{i}"),
            })
            .collect();
        let _ = update(&mut s, Message::Query("report".into()));
        let _ = command(&mut s, Command::OpenActions);
        s
    }

    #[test]
    fn application_action_names_are_searchable_and_keep_distinct_identities() {
        let mut s = application_actions(&["Nuova finestra privata", "New window", "New window"]);
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.label(Action::Desktop(0)), "Nuova finestra privata");
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Launch));
        assert_eq!(panel.action_for_command(Command::Secondary), None);
        let _ = update(&mut s, Message::ActionQuery("privata nuova".into()));
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.filtered_actions(), [Action::Desktop(0)]);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Desktop(0)));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(0)));
        assert_eq!(s.query, "report");
        let _ = update(&mut s, Message::ActionQuery("  NEW  window ".into()));
        assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::Desktop(1), Action::Desktop(2)]);
        let _ = update(&mut s, Message::ActionQuery("nwwin".into()));
        assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::Desktop(1), Action::Desktop(2)]);
    }

    #[test]
    fn long_action_menus_scroll_page_and_number_the_visible_window() {
        let names: Vec<_> = (0..12).map(|i| format!("Window {i}")).collect();
        let mut s = application_actions(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let _ = command(&mut s, Command::PageDown);
        assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 7);
        let _ = command(&mut s, Command::PageDown);
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.cursor.selected, 14);
        assert_eq!(panel.cursor.window(16), 7..15);
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(3)));
        let _ = command(&mut s, Command::Last);
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.cursor.window(16), 8..16);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Desktop(11)));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(4)));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(7)), Some(Action::Desktop(11)));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(8)), None);
        let _ = update(&mut s, Message::Wheel(1.0));
        assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 14);
        assert_eq!(s.cursor.selected, 0, "menu scrolling must leave the result selected");
        let _ = update(&mut s, Message::ActionQuery("Window 11".into()));
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.cursor.first, 0);
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(11)));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), None);
        let _ = command(&mut s, Command::Back);
        let panel = s.panel.as_ref().unwrap();
        assert_eq!(panel.cursor.window(16), 0..8);
        assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Launch));
        let _ = command(&mut s, Command::Last);
        let _ = command(&mut s, Command::PageUp);
        assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 8);
        let _ = command(&mut s, Command::First);
        assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 0);
    }

    #[test]
    fn invalid_application_action_keeps_the_menu_usable_and_does_not_record_usage() {
        let mut s = application_actions(&["New window"]);
        let invalid = Action::Desktop(usize::MAX);
        s.panel.as_mut().unwrap().actions.push(invalid);
        let target = s.selected().unwrap().target.clone();
        // While a file open is pending, application actions obey the same gate.
        s.opening = Some(("file:///synthetic/report.txt".into(), "report".into()));
        let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: invalid });
        assert!(!matches!(s.status, Status::Error(_)));
        s.opening = None;
        let _ = update(&mut s, Message::PanelClicked { target, action: invalid });
        assert!(matches!(s.status, Status::Error(_)));
        assert!(s.panel.is_some());
        assert_eq!(s.usage.boost(Kind::App, &s.apps[0].id, "report", usage::now()), 0);
        let _ = update(&mut s, Message::ActionQuery("missing".into()));
        let status = s.status.clone();
        let target = s.selected().unwrap().target.clone();
        let _ = update(&mut s, Message::PanelClicked { target, action: invalid });
        assert_eq!(s.status, status, "a filtered-out action cannot run from a queued click");
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
