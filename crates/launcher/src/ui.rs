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
use iced::{Background, Border, Color, Element, Length, Subscription, Task, Theme, padding};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};
use iced_layershell::to_layer_message;

use crate::activation::Activation;
use crate::apps::App;
use crate::backend::{ACTION_OPEN_PARENT, Backend, FileHit, RunError, SearchStatus, UiEvent};
use crate::calc;
use crate::details;
use crate::favorites::Favorites;
use crate::folders;
use crate::icons::{Icon, Icons};
use crate::material::Material;
use crate::model::{self, Action, ClickTracker, Command, Cursor, Item, Scope, Target};
use crate::open_with;
use crate::preview;
use crate::timing;
use crate::usage::{self, Kind, Usage};

pub const WIDTH: u32 = 720;
const PREVIEW_WIDTH: u32 = 980;
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
    pub const SURFACE: Color = Color::from_rgb8(0x17, 0x18, 0x1c);
    pub const GLASS: Color = Color::from_rgba8(0x17, 0x18, 0x1c, 0.88);
    pub const BORDER: Color = Color::from_rgba8(0xff, 0xff, 0xff, 0.10);
    pub const TEXT: Color = Color::from_rgb8(0xec, 0xea, 0xe6);
    pub const DIM: Color = Color::from_rgb8(0xb0, 0xb2, 0xbc);
    pub const FAINT: Color = Color::from_rgba8(0xff, 0xff, 0xff, 0.06);
    /// Oxidised iron.
    pub const ACCENT: Color = Color::from_rgb8(0xe0, 0x70, 0x3a);
    pub const SELECTED: Color = Color::from_rgba8(0xe0, 0x70, 0x3a, 0.18);
    pub const ERROR: Color = Color::from_rgb8(0xe8, 0x6a, 0x6a);
    /// Floating layers above the surface, such as the action panel.
    pub const RAISED: Color = Color::from_rgb8(0x22, 0x23, 0x29);
}

static UI_EVENTS: Mutex<Option<UnboundedReceiver<UiEvent>>> = Mutex::new(None);
static BOOT: Mutex<Option<(Boot, Material)>> = Mutex::new(None);
static MATERIAL_EVENTS: Mutex<Option<UnboundedReceiver<bool>>> = Mutex::new(None);

/// What the launcher starts with.
pub struct Boot {
    pub opaque: bool,
    pub activation: Activation,
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
                    let label = crate::matcher::fold(self.label(action));
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

struct FolderSearchFrame {
    query: String,
    scope: Scope,
    folder: Option<FileHit>,
    more: bool,
    files: Vec<FileHit>,
    cursor: Cursor,
    selected: Option<Target>,
}

pub struct Launcher {
    material: Material,
    glass: bool,
    activation: Activation,
    backend: Backend,
    apps: Vec<App>,
    usage: Usage,
    favorites: Favorites,
    /// The file being opened by the daemon. While set, further opens are
    /// ignored; its use is recorded once the daemon confirms.
    opening: Option<(String, String)>,
    launch_task: Option<iced::task::Handle>,
    /// A dispatched action lost its confirmation; never duplicate it in this session.
    unconfirmed_open: bool,
    /// Set by copy actions or captured input copy/cut shortcuts and never
    /// cleared: clipboard ownership changes cannot be observed here.
    clipboard_owner: bool,
    icons: Icons,
    query: String,
    scope: Scope,
    search_folder: Option<FileHit>,
    folder_more: bool,
    folder_search_history: Vec<FolderSearchFrame>,
    scope_generation: u64,
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
    application_picker: Option<open_with::Panel>,
    application_generation: u64,
    file_details: Option<details::Panel>,
    details_generation: u64,
    preview_enabled: bool,
    window_width: u32,
    text_preview: Option<preview::Panel>,
    preview_generation: u64,
    preview_request: Option<crate::backend::PreviewRequest>,
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
    MaterialChanged(bool),
    SurfaceResized((u32, u32)),
    ApplicationOpened { id: String, result: Result<(), String> },
    TimingFrame(std::time::Instant),
    ActionsFocusChecked(bool),
    Query(String),
    ContextQuery { generation: u64, query: String },
    Scope(Scope),
    ActionQuery(String),
    FolderBack { generation: u64 },
    PreviewLoaded { generation: u64, result: Result<crate::backend::PreviewData, String> },
    Preview { generation: u64, event: preview::Event },
    DetailsLoaded { generation: u64, result: Result<crate::backend::FileDetails, String> },
    Details { generation: u64, event: details::Event },
    ApplicationsLoaded { generation: u64, result: Result<Vec<crate::backend::ApplicationChoice>, String> },
    OpenWith { generation: u64, event: open_with::Event },
    ShowSearchStatus,
    ShowFolders,
    FolderShortcut(folders::Shortcut),
    Folder { generation: u64, event: folders::Event },
    RefreshSearchStatus,
    CloseSearchStatus,
    SearchStatusLoaded { generation: u64, result: Result<SearchStatus, String> },
    FolderListed { generation: u64, result: Result<(Vec<FileHit>, bool), String> },
    Files { generation: u64, result: Result<Option<Vec<FileHit>>, String> },
    Cmd(Command),
    Wheel(f32),
    ResultsWheel(f32),
    Selected(Target),
    Clicked(Target),
    ContextMenu(Target),
    PanelClicked { target: Target, action: Action },
    Opened(Result<(), RunError>),
    InputClipboard(Option<Box<Message>>),
    FocusLost,
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
    let activation = boot_data.activation.clone();
    let (material, material_events) = Material::new(!boot_data.opaque, (WIDTH, compact_height()));
    let shared_material = material.clone();
    *MATERIAL_EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(material_events);
    *UI_EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(events);
    *BOOT.lock().unwrap_or_else(|e| e.into_inner()) = Some((boot_data, material));
    iced_layershell::application(boot, namespace, update, view)
        .style(|_, _| iced::theme::Style { background_color: Color::TRANSPARENT, text_color: palette::TEXT })
        .theme(|state: &Launcher| material_theme(state.glass))
        .subscription(subscription)
        .settings(Settings {
            with_connection: Some(iced_layershell::reexport::WithConnection::from(move || {
                let connection = wayland_client::Connection::connect_to_env()?;
                activation.attach(connection.clone(), shared_material.clone());
                Ok(connection)
            })),
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
    let (Boot { activation, backend, apps, usage, favorites, initial_query, settings_path, .. }, material) =
        BOOT.lock().unwrap_or_else(|e| e.into_inner()).take().expect("boot data set once by run()");
    let first_run = !settings_path.exists();
    let mut state = Launcher {
        material,
        glass: false,
        activation,
        backend,
        apps,
        usage,
        favorites,
        opening: None,
        launch_task: None,
        unconfirmed_open: false,
        clipboard_owner: false,
        icons: Icons::new(ICON_SIZE as u16),
        query: String::new(),
        scope: Scope::All,
        search_folder: None,
        folder_more: false,
        folder_search_history: Vec::new(),
        scope_generation: 0,
        app_hits: Vec::new(),
        files: Vec::new(),
        items: Vec::new(),
        item_icons: Vec::new(),
        cursor: Cursor::new(VISIBLE_ROWS),
        clicks: ClickTracker::default(),
        panel: None,
        search_status: None,
        application_picker: None,
        application_generation: 0,
        file_details: None,
        details_generation: 0,
        preview_enabled: false,
        window_width: WIDTH,
        text_preview: None,
        preview_generation: 0,
        preview_request: None,
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
    let events = if timing::enabled() { event::listen_raw(on_event) } else { event::listen_with(on_event) };
    Subscription::batch([events, Subscription::run(ui_events), Subscription::run(material_events)])
}

fn on_event(event: Event, status: event::Status, _window: iced::window::Id) -> Option<Message> {
    match event {
        Event::Window(iced::window::Event::Resized(size) | iced::window::Event::Opened { size, .. }) => {
            Some(Message::SurfaceResized((size.width.round() as u32, size.height.round() as u32)))
        }
        Event::Window(iced::window::Event::RedrawRequested(at)) if timing::needs_frame() => {
            Some(Message::TimingFrame(at))
        }
        Event::Window(iced::window::Event::Unfocused) => Some(Message::FocusLost),
        // Read regardless of capture: the search field keeps focus and
        // swallows Escape, but navigation keys must still reach the list.
        Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. }) => {
            let command = key_message(&key, modifiers);
            if status == event::Status::Captured
                && modifiers.command()
                && matches!(key.to_latin(physical_key), Some('c' | 'x'))
            {
                Some(Message::InputClipboard(command.map(Box::new)))
            } else {
                command
            }
        }
        Event::Mouse(mouse::Event::WheelScrolled { delta }) if status != event::Status::Captured => {
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

fn material_events() -> BoxStream<'static, Message> {
    match MATERIAL_EVENTS.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(rx) => rx.map(Message::MaterialChanged).boxed(),
        None => stream::pending().boxed(),
    }
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
        self.items = if self.search_folder.is_some() {
            let mut files = self.files.clone();
            if !self.query.trim().is_empty() {
                model::order_files(&mut files, |file| {
                    self.usage.boost(Kind::File, &file.id, &self.query, usage::now())
                });
            }
            model::build_items(&[], &[], &files)
        } else if self.query.trim().is_empty() {
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
        let compact = self.search_folder.is_none()
            && self.query.trim().is_empty()
            && self.items.is_empty()
            && self.search_status.is_none()
            && self.folder_settings.is_none()
            && self.application_picker.is_none()
            && self.file_details.is_none()
            && !self.preview_enabled
            && !matches!(self.status, Status::Error(_) | Status::Notice(_) | Status::Searching);
        let width = if self.preview_visible() { PREVIEW_WIDTH } else { WIDTH };
        if compact == self.compact && width == self.window_width {
            return Task::none();
        }
        self.compact = compact;
        self.window_width = width;
        let h = if compact { compact_height() } else { full_height() };
        Task::done(Message::SizeChange((width, h)))
    }

    fn selected(&self) -> Option<&Item> {
        self.items.get(self.cursor.selected)
    }

    fn perform(&mut self, action: Action) -> Task<Message> {
        self.clicks = ClickTracker::default();
        let Some(item) = self.selected().cloned() else { return Task::none() };
        let opens =
            matches!(action, Action::Launch | Action::Desktop(_) | Action::Open | Action::Reveal | Action::OpenWith);
        if (opens || matches!(action, Action::Details | Action::Preview | Action::BrowseFolder))
            && self.opening.is_some()
        {
            return Task::none(); // one open at a time; the launcher exits after it
        }
        if opens && self.unconfirmed_open {
            self.status = Status::Error(UNCONFIRMED_OPEN.into());
            return self.resize();
        }
        match (action, &item.target) {
            (Action::Launch | Action::Desktop(_), Target::App(i)) => {
                let app = self.apps[*i].clone();
                let valid = match action {
                    Action::Desktop(index) => app.action_argv(index),
                    _ => app.argv(),
                };
                if let Err(error) = valid {
                    self.status = Status::Error(format!("Cannot launch {}: {error}", app.name));
                    return self.resize();
                }
                let id = app.id.clone();
                let activation = self.activation.clone();
                self.opening = Some((id.clone(), self.query.clone()));
                self.status = Status::Notice("Opening application…".into());
                let (task, handle) = Task::perform(
                    async move {
                        let app_id =
                            if app.terminal { String::new() } else { app.id.trim_end_matches(".desktop").to_string() };
                        let token = activation.token(app_id).await;
                        let result = match action {
                            Action::Desktop(index) => crate::launch::launch_action(&app, index, token.as_deref()),
                            _ => crate::launch::launch(&app, token.as_deref()),
                        };
                        result.map_err(|error| format!("Cannot launch {}: {error}", app.name))
                    },
                    move |result| Message::ApplicationOpened { id: id.clone(), result },
                )
                .abortable();
                self.launch_task = Some(handle.abort_on_drop());
                task
            }
            (Action::Open | Action::Reveal, Target::File(f)) => {
                let act = if action == Action::Reveal { ACTION_OPEN_PARENT } else { "" };
                self.opening = Some((f.id.clone(), self.query.clone()));
                self.status = Status::Notice("Opening…".into());
                let (task, handle) =
                    Task::perform(self.backend.clone().run(f.id.clone(), act), Message::Opened).abortable();
                self.launch_task = Some(handle.abort_on_drop());
                task
            }
            (Action::BrowseFolder, Target::File(folder)) if folder.is_dir && folder.path.is_some() => {
                self.enter_folder_search(folder.clone())
            }
            (Action::Preview, Target::File(file)) if !file.is_dir && file.path.is_some() => {
                self.start_preview(file.clone())
            }
            (Action::Details, Target::File(file)) if !file.is_dir && file.path.is_some() => {
                self.panel = None;
                self.file_details = Some(details::Panel::new(file.clone()));
                let load = self.request_file_details();
                Task::batch([load, self.resize()])
            }
            (Action::OpenWith, Target::File(file)) if !file.is_dir && file.path.is_some() => {
                self.panel = None;
                self.application_picker = Some(open_with::Panel::new(file.clone()));
                let load = self.request_applications();
                Task::batch([load, self.resize(), widget::operation::focus(open_with::input_id())])
            }
            (Action::CopyPath, Target::File(f)) => match &f.path {
                Some(p) => match p.to_str() {
                    Some(path) => self.copy(path.to_owned(), "path"),
                    None => self.copy(f.id.clone(), "file URL"),
                },
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
        Task::batch([self.write_clipboard(s), widget::operation::focus(search_id())])
    }

    /// The launcher then serves the clipboard, so focus loss no longer closes it.
    fn write_clipboard(&mut self, s: String) -> Task<Message> {
        self.clipboard_owner = true;
        iced::clipboard::write(s)
    }

    fn primary(&self) -> Option<Action> {
        self.selected().and_then(|i| self.actions_for(&i.target).first().copied())
    }

    fn file_query(&self) -> bool {
        if self.search_folder.is_some() {
            let query = self.query.trim();
            query.is_empty() || query.chars().count() >= MIN_FILE_QUERY
        } else {
            self.scope.file_query(&self.query)
        }
    }

    fn request_files(&self) -> Task<Message> {
        let (backend, query, folder, generation) = (
            self.backend.clone(),
            self.query.clone(),
            self.search_folder.as_ref().map(|folder| folder.id.clone()),
            self.generation,
        );
        if query.trim().is_empty()
            && let Some(folder) = folder.as_ref()
        {
            return Task::perform(backend.list_directory(folder.clone()), move |result| Message::FolderListed {
                generation,
                result,
            });
        }
        let deliver = move |result| Message::Files { generation, result };
        if let Some(folder) = folder {
            Task::perform(backend.search_within(folder, query), deliver)
        } else {
            Task::perform(backend.search(query), deliver)
        }
    }

    fn refresh_folder(&mut self) -> Task<Message> {
        if !self.file_query() {
            return Task::none();
        }
        if self.preview_enabled {
            self.cancel_preview(true);
        }
        self.generation += 1;
        self.status = Status::Searching;
        Task::batch([self.request_files(), self.resize()])
    }

    fn enter_folder_search(&mut self, folder: FileHit) -> Task<Message> {
        if self.folder_search_history.len() >= 32 {
            self.status = Status::Error("Return to a previous folder before narrowing further.".into());
            return Task::none();
        }
        self.folder_search_history.push(FolderSearchFrame {
            query: self.query.clone(),
            scope: self.scope,
            folder: self.search_folder.take(),
            more: self.folder_more,
            files: self.files.clone(),
            cursor: self.cursor,
            selected: self.selected().map(|item| item.target.clone()),
        });
        self.search_folder = Some(folder);
        self.scope_generation += 1;
        self.scope = Scope::Files;
        let query = update(self, Message::Query(String::new()));
        Task::batch([query, widget::operation::focus(search_id())])
    }

    fn restore_folder_search(&mut self, frame: FolderSearchFrame) {
        self.query = frame.query;
        self.scope = frame.scope;
        self.search_folder = frame.folder;
        self.folder_more = frame.more;
        self.files = frame.files;
        self.generation += 1;
        self.scope_generation += 1;
        self.panel = None;
        self.status = Status::Idle;
        self.app_hits = if self.scope.includes_apps() && !calc::is_explicit(&self.query) {
            model::match_apps(&self.apps, &self.query, |app| {
                self.usage.boost(Kind::App, &app.id, &self.query, usage::now())
            })
        } else {
            Vec::new()
        };
        self.rebuild(false);
        self.cursor = frame.cursor;
        self.cursor.select(self.cursor.selected, self.items.len());
        if let Some(target) = frame.selected {
            self.cursor.select_target(&target, &self.items);
        }
    }

    fn back_folder_search(&mut self) -> Task<Message> {
        let Some(frame) = self.folder_search_history.pop() else { return Task::none() };
        self.restore_folder_search(frame);
        let refresh = if self.file_query() {
            self.status = Status::Searching;
            self.request_files()
        } else {
            Task::none()
        };
        Task::batch([
            refresh,
            self.resize(),
            widget::operation::focus(search_id()),
            widget::operation::move_cursor_to_end(search_id()),
        ])
    }

    fn preview_visible(&self) -> bool {
        self.preview_enabled
            && self.file_details.is_none()
            && self.application_picker.is_none()
            && self.folder_settings.is_none()
            && self.search_status.is_none()
    }

    fn preview_context(&self) -> (Option<Target>, bool) {
        (self.selected().map(|item| item.target.clone()), self.preview_visible())
    }

    fn cancel_preview(&mut self, clear: bool) {
        if let Some(request) = self.preview_request.take() {
            request.cancel();
        }
        self.preview_generation += 1;
        if clear {
            self.text_preview = None;
        }
    }

    fn sync_preview(&mut self) -> Task<Message> {
        if !self.preview_visible() {
            return Task::none();
        }
        let file = self.selected().and_then(|item| match &item.target {
            Target::File(file) if !file.is_dir && file.path.is_some() => Some(file.clone()),
            _ => None,
        });
        let Some(file) = file else {
            if self.text_preview.is_some() || self.preview_request.is_some() {
                self.cancel_preview(true);
            }
            return Task::none();
        };
        if self.text_preview.as_ref().is_some_and(|panel| panel.file.id == file.id) {
            return Task::none();
        }
        self.text_preview = Some(preview::Panel::new(file));
        Task::batch([
            self.request_preview(),
            widget::operation::snap_to(preview::scroll_id(), widget::operation::RelativeOffset::START),
        ])
    }

    fn start_preview(&mut self, file: FileHit) -> Task<Message> {
        self.panel = None;
        self.file_details = None;
        self.details_generation += 1;
        self.preview_enabled = true;
        let load = if self.cursor.select_target(&Target::File(file.clone()), &self.items) {
            self.text_preview = Some(preview::Panel::new(file));
            self.request_preview()
        } else {
            self.text_preview = None;
            self.preview_generation += 1;
            self.status = Status::Notice("File no longer in results. Select a file to preview.".into());
            Task::none()
        };
        Task::batch([load, self.resize(), widget::operation::focus(search_id())])
    }

    fn request_preview(&mut self) -> Task<Message> {
        let Some(panel) = &self.text_preview else { return Task::none() };
        let id = panel.file.id.clone();
        if let Some(previous) = self.preview_request.take() {
            previous.cancel();
        }
        let request = crate::backend::PreviewRequest::default();
        self.preview_request = Some(request.clone());
        self.preview_generation += 1;
        let generation = self.preview_generation;
        Task::perform(self.backend.clone().text_preview(id, request), move |result| Message::PreviewLoaded {
            generation,
            result,
        })
    }

    fn preview_effect(&mut self, effect: preview::Effect) -> Task<Message> {
        use widget::operation::{AbsoluteOffset, RelativeOffset};
        match effect {
            preview::Effect::None => Task::none(),
            preview::Effect::Close => {
                self.preview_enabled = false;
                self.cancel_preview(true);
                timing::event("preview_closed");
                Task::batch([self.resize(), widget::operation::focus(search_id())])
            }
            preview::Effect::Refresh => Task::batch([
                self.request_preview(),
                widget::operation::snap_to(preview::scroll_id(), RelativeOffset::START),
            ]),
            preview::Effect::Copy(location) => self.write_clipboard(location),
            preview::Effect::Scroll(y) => {
                widget::operation::scroll_by(preview::scroll_id(), AbsoluteOffset { x: 0.0, y })
            }
        }
    }

    fn request_file_details(&mut self) -> Task<Message> {
        let Some(panel) = &self.file_details else { return Task::none() };
        let id = panel.file.id.clone();
        self.details_generation += 1;
        let generation = self.details_generation;
        Task::perform(self.backend.clone().file_details(id), move |result| Message::DetailsLoaded {
            generation,
            result,
        })
    }

    fn details_effect(&mut self, effect: details::Effect) -> Task<Message> {
        use widget::operation::{AbsoluteOffset, RelativeOffset};
        match effect {
            details::Effect::None => Task::none(),
            details::Effect::Close => {
                if let Some(panel) = self.file_details.take() {
                    self.cursor.select_target(&Target::File(panel.file), &self.items);
                }
                self.details_generation += 1;
                Task::batch([self.resize(), widget::operation::focus(search_id())])
            }
            details::Effect::Preview => {
                let Some(panel) = &self.file_details else { return Task::none() };
                self.start_preview(panel.file.clone())
            }
            details::Effect::Refresh => self.request_file_details(),
            details::Effect::Copy(location) => self.write_clipboard(location),
            details::Effect::Scroll(y) => {
                widget::operation::scroll_by(details::scroll_id(), AbsoluteOffset { x: 0.0, y })
            }
            details::Effect::Edge(end) => widget::operation::snap_to(
                details::scroll_id(),
                if end { RelativeOffset::END } else { RelativeOffset::START },
            ),
        }
    }

    fn request_applications(&mut self) -> Task<Message> {
        let Some(panel) = &self.application_picker else { return Task::none() };
        let id = panel.file.id.clone();
        self.application_generation += 1;
        let generation = self.application_generation;
        Task::perform(self.backend.clone().applications(id), move |result| Message::ApplicationsLoaded {
            generation,
            result,
        })
    }

    fn application_effect(&mut self, effect: open_with::Effect) -> Task<Message> {
        match effect {
            open_with::Effect::None => Task::none(),
            open_with::Effect::Focus => widget::operation::focus(open_with::input_id()),
            open_with::Effect::Close => {
                if let Some(panel) = self.application_picker.take() {
                    self.cursor.select_target(&Target::File(panel.file), &self.items);
                }
                self.application_generation += 1;
                Task::batch([self.resize(), widget::operation::focus(search_id())])
            }
            open_with::Effect::Reload => {
                if self.opening.is_some() || !self.application_picker.as_mut().is_some_and(|panel| panel.loading()) {
                    return Task::none();
                }
                Task::batch([self.request_applications(), widget::operation::focus(open_with::input_id())])
            }
            open_with::Effect::Open(app) => {
                if self.opening.is_some() || self.unconfirmed_open {
                    return Task::none();
                }
                let Some(panel) = &mut self.application_picker else { return Task::none() };
                let id = panel.file.id.clone();
                panel.opening();
                self.opening = Some((id.clone(), self.query.clone()));
                self.status = Status::Notice("Opening…".into());
                let (task, handle) = Task::perform(self.backend.clone().run_with(id, app), Message::Opened).abortable();
                self.launch_task = Some(handle.abort_on_drop());
                task
            }
        }
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
                let root = self.folder_search_history.drain(..).next();
                if let Some(root) = root {
                    self.restore_folder_search(root);
                }
                let query = self.deferred_query.take().unwrap_or_else(|| self.query.clone());
                let search = update(self, Message::Query(query));
                self.status = Status::Notice("Folders applied. Indexing continues in the background.".into());
                Task::batch([task, search, self.resize(), widget::operation::focus(search_id())])
            }
        }
    }
}

fn update(s: &mut Launcher, message: Message) -> Task<Message> {
    let previous = s.preview_context();
    let query_boundary = matches!(&message, Message::Query(_));
    let task = update_inner(s, message);
    if previous.1 && !s.preview_visible() && s.preview_enabled {
        let loading = s.preview_request.is_some();
        s.cancel_preview(loading);
    }
    if previous != s.preview_context() || query_boundary {
        let preview = s.sync_preview();
        Task::batch([task, preview, s.resize()])
    } else {
        task
    }
}

fn update_inner(s: &mut Launcher, message: Message) -> Task<Message> {
    if let Message::SurfaceResized(size) = message {
        s.material.resize(size);
        timing::surface_resized(size);
        return Task::none();
    }
    if let Message::MaterialChanged(enabled) = message {
        s.glass = enabled;
        timing::event(if enabled { "material_blur_requested" } else { "material_opaque" });
        return Task::none();
    }
    if let Message::ActionsFocusChecked(focused) = message {
        if focused && s.panel.is_some() {
            timing::event("actions_focused");
        }
        return Task::none();
    }
    if let Message::TimingFrame(at) = message {
        let visible = s.panel.is_none()
            && s.file_details.is_none()
            && s.application_picker.is_none()
            && s.folder_settings.is_none()
            && s.search_status.is_none();
        timing::frame(at, s.generation, visible);
        return Task::none();
    }
    if (s.application_picker.is_some() || s.file_details.is_some())
        && matches!(
            message,
            Message::Query(_)
                | Message::ContextQuery { .. }
                | Message::Scope(_)
                | Message::ActionQuery(_)
                | Message::FolderBack { .. }
                | Message::Selected(_)
                | Message::Clicked(_)
                | Message::ContextMenu(_)
                | Message::PanelClicked { .. }
                | Message::ShowSearchStatus
                | Message::ShowFolders
                | Message::ResultsWheel(_)
        )
    {
        return Task::none();
    }
    if s.folder_settings.is_some()
        && matches!(
            message,
            Message::Query(_)
                | Message::ContextQuery { .. }
                | Message::Scope(_)
                | Message::ActionQuery(_)
                | Message::FolderBack { .. }
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
                | Message::ContextQuery { .. }
                | Message::Scope(_)
                | Message::ActionQuery(_)
                | Message::FolderBack { .. }
                | Message::Selected(_)
                | Message::Clicked(_)
                | Message::ContextMenu(_)
                | Message::PanelClicked { .. }
        )
    {
        return Task::none();
    }
    match message {
        Message::PreviewLoaded { generation, result } => {
            if generation == s.preview_generation
                && let Some(panel) = &mut s.text_preview
            {
                panel.loaded(result);
                s.preview_request = None;
                timing::event("preview_loaded");
            }
            Task::none()
        }
        Message::Preview { generation, event } => {
            if generation != s.preview_generation {
                return Task::none();
            }
            if !s.preview_visible() {
                return Task::none();
            }
            if matches!(event, preview::Event::Back) {
                return s.preview_effect(preview::Effect::Close);
            }
            let Some(panel) = &mut s.text_preview else { return Task::none() };
            let effect = panel.event(event);
            s.preview_effect(effect)
        }
        Message::RefreshSearchStatus
            if s.search_folder.is_some()
                && s.file_details.is_none()
                && s.application_picker.is_none()
                && s.folder_settings.is_none()
                && s.search_status.is_none() =>
        {
            s.refresh_folder()
        }
        Message::RefreshSearchStatus if s.preview_visible() && s.text_preview.is_some() => {
            let effect = s.text_preview.as_mut().unwrap().event(preview::Event::Refresh);
            s.preview_effect(effect)
        }
        Message::Wheel(_) if s.preview_visible() => Task::none(),
        Message::ResultsWheel(_) if !s.preview_visible() => Task::none(),
        Message::ResultsWheel(dy) => move_results(s, dy),
        Message::FolderBack { generation } => {
            if generation == s.scope_generation {
                s.back_folder_search()
            } else {
                Task::none()
            }
        }
        Message::DetailsLoaded { generation, result } => {
            if generation == s.details_generation
                && let Some(panel) = &mut s.file_details
            {
                panel.loaded(result);
            }
            Task::none()
        }
        Message::Details { generation, event } => {
            if generation != s.details_generation {
                return Task::none();
            }
            let Some(panel) = &mut s.file_details else { return Task::none() };
            let effect = panel.event(event);
            s.details_effect(effect)
        }
        Message::RefreshSearchStatus if s.file_details.is_some() => {
            let effect = s.file_details.as_mut().unwrap().event(details::Event::Refresh);
            s.details_effect(effect)
        }
        Message::Wheel(_) if s.file_details.is_some() => Task::none(),
        Message::ApplicationsLoaded { generation, result } => {
            if generation == s.application_generation
                && let Some(panel) = &mut s.application_picker
            {
                panel.loaded(result, &mut s.icons);
                timing::event("applications_loaded");
                return widget::operation::focus(open_with::input_id());
            }
            Task::none()
        }
        Message::OpenWith { generation, event } => {
            if generation != s.application_generation {
                return Task::none();
            }
            if s.opening.is_some() && !matches!(event, open_with::Event::Back) {
                return Task::none();
            }
            let Some(panel) = &mut s.application_picker else { return Task::none() };
            if matches!(event, open_with::Event::Query(_)) {
                timing::event("application_input");
            }
            let effect = panel.event(event);
            s.application_effect(effect)
        }
        Message::RefreshSearchStatus if s.application_picker.is_some() => {
            s.application_effect(open_with::Effect::Reload)
        }
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
        Message::ContextQuery { generation, query } => {
            if generation == s.scope_generation {
                update(s, Message::Query(query))
            } else {
                Task::none()
            }
        }
        Message::Query(q) => {
            if s.preview_enabled {
                s.cancel_preview(true);
            }
            s.clicks = ClickTracker::default();
            // Editing the main field leaves the action panel and starts a new search.
            s.panel = None;
            s.query = q;
            s.generation += 1;
            timing::begin(s.generation);
            let (usage, now) = (&s.usage, usage::now());
            let q = s.query.as_str();
            // An explicit calculator query stays in that mode while incomplete.
            let calculator = s.search_folder.is_none() && calc::is_explicit(q);
            s.app_hits = if calculator || !s.scope.includes_apps() {
                Vec::new()
            } else {
                model::match_apps(&s.apps, q, |a| usage.boost(Kind::App, &a.id, q, now))
            };
            let file_query = s.file_query();
            // Results of the previous query must not stay actionable while
            // this one is searched.
            s.files.clear();
            s.folder_more = false;
            s.status = if file_query { Status::Searching } else { Status::Idle };
            s.rebuild(false);
            if !file_query {
                timing::ready(s.generation, s.items.len(), s.files.len(), true);
            }
            let search = if file_query { s.request_files() } else { Task::none() };
            Task::batch([search, s.resize()])
        }
        Message::Scope(_) if s.search_folder.is_some() => Task::none(),
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
                timing::event("action_input");
            }
            Task::none()
        }
        Message::FolderListed { generation, result } => {
            if generation != s.generation {
                return Task::none();
            }
            let result = result.map(|(files, more)| {
                s.folder_more = more;
                Some(files)
            });
            update(s, Message::Files { generation, result })
        }
        Message::Files { generation, result } => {
            if generation != s.generation {
                return Task::none(); // a newer query is in flight
            }
            let success = result.is_ok();
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
                    s.status = Status::Error(if s.search_folder.is_some() {
                        e
                    } else {
                        format!("File search unavailable: {e}")
                    });
                }
            }
            let had_panel = s.panel.is_some();
            s.rebuild(true);
            timing::ready(generation, s.items.len(), s.files.len(), success);
            let focus =
                if had_panel && s.panel.is_none() { widget::operation::focus(search_id()) } else { Task::none() };
            Task::batch([s.resize(), focus, s.sync_preview()])
        }
        Message::Cmd(cmd) => command(s, cmd),
        Message::Wheel(dy) => {
            s.clicks = ClickTracker::default();
            if let Some(panel) = &mut s.application_picker {
                if s.opening.is_none() {
                    panel.wheel(if dy > 0.0 { -1 } else { 1 });
                }
                return Task::none();
            }
            if s.search_status.is_some() || s.folder_settings.is_some() {
                return Task::none(); // the status scrollable handles its own wheel events
            }
            move_results(s, dy)
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
        Message::ApplicationOpened { id, result } => {
            if !s.opening.as_ref().is_some_and(|(pending, _)| *pending == id) {
                return Task::none();
            }
            let (_, query) = s.opening.take().unwrap();
            s.launch_task = None;
            match result {
                Ok(()) => {
                    s.usage.record(Kind::App, &id, &query, usage::now());
                    iced::exit()
                }
                Err(error) => {
                    s.status = Status::Error(error);
                    s.resize()
                }
            }
        }
        Message::Opened(Ok(())) => {
            if let Some((id, query)) = s.opening.take() {
                s.launch_task = None;
                s.usage.record(Kind::File, &id, &query, usage::now());
                return iced::exit();
            }
            Task::none()
        }
        Message::Opened(Err(e)) => {
            if s.opening.take().is_none() {
                return Task::none();
            }
            s.launch_task = None;
            s.status = match e {
                RunError::Failed(message) => Status::Error(format!("Cannot open: {message}")),
                RunError::Unconfirmed(_) => {
                    s.unconfirmed_open = true;
                    Status::Error(UNCONFIRMED_OPEN.into())
                }
            };
            if let Some(panel) = &mut s.application_picker
                && let Status::Error(error) = &s.status
            {
                panel.failed(error.clone());
            }
            s.resize()
        }
        Message::Ui(UiEvent::Toggle) => {
            s.launch_task = None;
            iced::exit()
        }
        Message::InputClipboard(command) => {
            s.clipboard_owner = true;
            command.map_or_else(Task::none, |message| update_inner(s, *message))
        }
        // An open in flight finishes and exits on its own (or shows its error);
        // folder settings may hold unsaved edits or be saving; a copy is pasted
        // from the launcher after the click into the destination window.
        Message::FocusLost if s.opening.is_none() && s.folder_settings.is_none() && !s.clipboard_owner => {
            s.launch_task = None;
            iced::exit()
        }
        // Layer-shell requests generated by `to_layer_message` are handled
        // by the runtime before they reach here.
        _ => Task::none(),
    }
}

fn move_results(s: &mut Launcher, dy: f32) -> Task<Message> {
    let delta = if dy > 0.0 { -1 } else { 1 };
    if let Some(panel) = &mut s.panel {
        panel.cursor.move_by(delta, panel.filtered_actions().len());
    } else {
        s.cursor.move_by(delta, s.items.len());
    }
    Task::none()
}

fn command(s: &mut Launcher, cmd: Command) -> Task<Message> {
    let previous = s.preview_context();
    let task = command_inner(s, cmd);
    if previous.1 && !s.preview_visible() && s.preview_enabled {
        let loading = s.preview_request.is_some();
        s.cancel_preview(loading);
    }
    if previous != s.preview_context() {
        let preview = s.sync_preview();
        Task::batch([task, preview, s.resize()])
    } else {
        task
    }
}

fn command_inner(s: &mut Launcher, cmd: Command) -> Task<Message> {
    s.clicks = ClickTracker::default();
    if let Some(panel) = &mut s.file_details {
        let effect = panel.command(cmd);
        return s.details_effect(effect);
    }
    if let Some(panel) = &mut s.application_picker {
        if s.opening.is_some() && cmd != Command::Back {
            return Task::none();
        }
        let effect = panel.command(cmd);
        return s.application_effect(effect);
    }
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
    match cmd {
        Command::ScrollPreview(_) => {
            if s.preview_visible()
                && let Some(panel) = &mut s.text_preview
            {
                let effect = panel.command(cmd);
                return s.preview_effect(effect);
            }
            return Task::none();
        }
        Command::BrowseFolder => return s.perform(Action::BrowseFolder),
        Command::BackFolder => return s.back_folder_search(),
        _ => {}
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
                timing::event("action_filter_cleared");
                return widget::operation::focus(actions_id());
            }
            Command::Back | Command::ToggleActions => {
                s.panel = None;
                timing::event("actions_closed");
                return widget::operation::focus(search_id());
            }
            _ => {}
        }
        return Task::none();
    }
    if s.preview_visible() {
        if cmd == Command::Back {
            return s.preview_effect(preview::Effect::Close);
        }
        if cmd == Command::CopyPath
            && let Some(panel) = &mut s.text_preview
        {
            let effect = panel.event(preview::Event::CopyLocation);
            return s.preview_effect(effect);
        }
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
                let focus = widget::operation::focus(actions_id());
                return if timing::enabled() {
                    focus.chain(widget::operation::is_focused(actions_id()).map(Message::ActionsFocusChecked))
                } else {
                    focus
                };
            }
        }
        Command::BrowseFolder | Command::BackFolder | Command::ScrollPreview(_) => {
            unreachable!("handled before action navigation")
        }
        Command::Back if s.search_folder.is_some() => {
            if s.query.is_empty() {
                return s.back_folder_search();
            }
            let clear = update(s, Message::Query(String::new()));
            return Task::batch([clear, widget::operation::focus(search_id())]);
        }
        Command::Back => {
            if s.query.is_empty() {
                s.launch_task = None;
                return iced::exit();
            }
            // Apply the clear synchronously so it cannot arrive in a later context.
            let clear = update(s, Message::Query(String::new()));
            return Task::batch([clear, widget::operation::focus(search_id())]);
        }
    }
    Task::none()
}

// ---------------------------------------------------------------- view

fn view(s: &Launcher) -> Element<'_, Message> {
    crate::material::renderer::opaque_fallback(view_content(s), material_theme(false))
}

fn view_content(s: &Launcher) -> Element<'_, Message> {
    if let Some(panel) = &s.file_details {
        let generation = s.details_generation;
        return panel.view().map(move |event| Message::Details { generation, event });
    }
    if let Some(panel) = &s.application_picker {
        let generation = s.application_generation;
        return crate::shortcuts::capture_numbered(
            panel.view(s.opening.is_some()).map(move |event| Message::OpenWith { generation, event }),
        );
    }
    if let Some(panel) = &s.folder_settings {
        let generation = s.folder_generation;
        return panel.view().map(move |event| Message::Folder { generation, event });
    }
    if let Some(status) = &s.search_status {
        return search_status_view(status);
    }
    let placeholder = if s.search_folder.is_some() {
        "Search this folder and subfolders…"
    } else {
        match s.scope {
            Scope::All => "Search applications and files…",
            Scope::Applications => "Search applications…",
            Scope::Files => "Search indexed files…",
        }
    };
    let scope_generation = s.scope_generation;
    let input = text_input(placeholder, &s.query)
        .id(search_id())
        .on_input(move |query| Message::ContextQuery { generation: scope_generation, query })
        .size(if s.search_folder.is_some() { 16 } else { 20 })
        .padding(if s.search_folder.is_some() { [3, 8] } else { [14, 8] })
        .style(|_, _| text_input::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            icon: palette::DIM,
            placeholder: palette::DIM,
            value: palette::TEXT,
            selection: palette::SELECTED,
        });
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
    let header: Element<'_, Message> = if let Some(folder) = &s.search_folder {
        let generation = s.scope_generation;
        let path = folder.path.as_deref().and_then(|path| path.to_str()).unwrap_or(&folder.id);
        let location = widget::tooltip(
            text(ellipsize_middle(path, 62)).size(11).color(palette::DIM),
            container(text(path).size(12).wrapping(text::Wrapping::WordOrGlyph))
                .padding(8)
                .max_width(600)
                .style(surface_style),
            widget::tooltip::Position::Bottom,
        );
        column![
            row![
                button(hint("Alt+←", "Back")).on_press(Message::FolderBack { generation }).style(status_button_style),
                location,
                space::horizontal(),
                button(hint("Ctrl+R", "Refresh")).on_press(Message::RefreshSearchStatus).style(status_button_style)
            ]
            .spacing(6)
            .align_y(Vertical::Center),
            input
        ]
        .spacing(2)
        .into()
    } else {
        row![input, scopes, separator, index_status, folders].spacing(4).align_y(Vertical::Center).into()
    };
    let header = container(header).padding([0, 8]).height(INPUT_HEIGHT).align_y(Vertical::Center);

    let mut body = column![header, divider()];
    if !s.compact {
        let results: Element<'_, Message> = if s.preview_visible() {
            let generation = s.preview_generation;
            let list = mouse_area(container(results(s)).width(Length::Fill).height(Length::Fill)).on_scroll(|delta| {
                let y = match delta {
                    ScrollDelta::Lines { y, .. } => y,
                    ScrollDelta::Pixels { y, .. } => y / ROW_HEIGHT,
                };
                Message::ResultsWheel(y)
            });
            let preview = match &s.text_preview {
                Some(panel) => panel.view(),
                None => preview::empty_view(),
            }
            .map(move |event| Message::Preview { generation, event });
            row![
                container(list).width(Length::FillPortion(2)),
                container(space()).width(1).height(Length::Fill).style(|_| container::Style {
                    background: Some(Background::Color(palette::FAINT)),
                    ..Default::default()
                }),
                container(preview).width(Length::FillPortion(3))
            ]
            .into()
        } else {
            results(s)
        };
        body = body.push(container(results).height(ROW_HEIGHT * VISIBLE_ROWS as f32 + PAD).padding([PAD / 2.0, 0.0]));
        body = body.push(divider());
    }
    body = body.push(footer(s));

    let surface = container(body).padding(PAD).width(Length::Fill).height(Length::Fill).style(surface_style);

    crate::shortcuts::capture_numbered(match (&s.panel, s.selected()) {
        (Some(p), Some(item)) => stack![surface, action_panel(p, &item.target)].into(),
        _ => surface.into(),
    })
}

fn material_theme(glass: bool) -> Theme {
    let mut colors = Theme::Dark.palette();
    colors.background = if glass { palette::GLASS } else { palette::SURFACE };
    Theme::custom("FerroSeek", colors)
}

pub(crate) fn surface_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme.palette().background)),
        border: Border { color: palette::BORDER, width: 1.0, radius: (crate::material::RADIUS as f32).into() },
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
            Status::Searching if s.search_folder.is_some() && s.query.trim().is_empty() => ("Loading folder…", ""),
            Status::Searching => ("Searching…", ""),
            Status::Error(_) if s.search_folder.is_some() => {
                ("Folder unavailable", "Refresh to retry, or return to your previous search.")
            }
            _ if s.search_folder.is_some() && s.query.trim().is_empty() => {
                ("No indexed items in this folder", "Refresh, or type at least 2 characters to search subfolders.")
            }
            _ if s.search_folder.is_some() && s.query.trim().chars().count() < MIN_FILE_QUERY => {
                ("Search this folder and subfolders", "Type at least 2 characters to search indexed files here.")
            }
            _ if s.search_folder.is_some() => {
                ("No matching indexed files", "Try another name or return to your previous search.")
            }
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
        Status::Searching if s.search_folder.is_some() && s.query.trim().is_empty() => {
            ("Loading folder…".into(), palette::DIM)
        }
        Status::Searching => ("Searching…".to_string(), palette::DIM),
        Status::Idle if s.search_folder.is_some() && s.query.trim().is_empty() => (
            if s.folder_more {
                format!("First {} indexed items · Type to search", s.items.len())
            } else {
                format!("{} indexed items", s.items.len())
            },
            palette::DIM,
        ),
        Status::Idle if s.search_folder.is_none() && calc::is_explicit(&s.query) => {
            ("Calculator".to_string(), palette::DIM)
        }
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
        if acts.contains(&Action::BrowseFolder) {
            hints = hints.push(hint("Alt+→", "Browse here"));
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
        hints = hints.push(
            container(hint(
                "Esc",
                if s.preview_visible() {
                    "Hide preview"
                } else if !s.query.is_empty() {
                    "Clear search"
                } else if s.search_folder.is_some() {
                    "Previous search"
                } else {
                    "Close"
                },
            ))
            .padding([3, 6]),
        );
    }
    container(
        row![text(msg).size(12).color(color).wrapping(text::Wrapping::None), space::horizontal(), hints]
            .align_y(Vertical::Center)
            .padding(padding::left(16).right(10)),
    )
    .height(FOOTER_HEIGHT)
    .align_y(Vertical::Center)
    .style(|_| container::Style {
        background: Some(Background::Color(palette::FAINT)),
        border: Border { radius: 9.0.into(), ..Default::default() },
        ..Default::default()
    })
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
        // No shadow: iced_tiny_skia 0.14 paints it unclipped on partial redraws
        // (cursor blink), darkening the panel. RAISED and the border lift it.
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
    fn material_changes_preserve_the_current_search_and_actions() {
        let mut state = application_actions(&["New window"]);
        state.query = "report".into();
        state.panel.as_mut().unwrap().query = "window".into();
        for enabled in [true, false, true] {
            let _ = update(&mut state, Message::MaterialChanged(enabled));
            assert_eq!(state.query, "report");
            assert_eq!(state.panel.as_ref().unwrap().query, "window");
            assert_eq!(state.cursor.selected, 0);
            assert_eq!(material_theme(state.glass).palette().background.a < 1.0, enabled);
        }
    }

    #[test]
    fn material_keeps_small_text_readable_over_light_and_dark_backdrops() {
        fn over(top: Color, bottom: Color) -> Color {
            Color::from_rgb(
                top.r * top.a + bottom.r * (1.0 - top.a),
                top.g * top.a + bottom.g * (1.0 - top.a),
                top.b * top.a + bottom.b * (1.0 - top.a),
            )
        }
        fn luminance(color: Color) -> f32 {
            let linear = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            linear(color.r) * 0.2126 + linear(color.g) * 0.7152 + linear(color.b) * 0.0722
        }
        for glass in [false, true] {
            for backdrop in [Color::WHITE, Color::BLACK] {
                let surface = over(material_theme(glass).palette().background, backdrop);
                for background in
                    [surface, over(palette::FAINT, surface), over(palette::SELECTED, surface), palette::RAISED]
                {
                    for foreground in [palette::TEXT, palette::DIM] {
                        let ratio = (luminance(foreground) + 0.05) / (luminance(background) + 0.05);
                        assert!(ratio >= 4.5, "small text contrast {ratio} on {background:?}");
                    }
                }
            }
        }
    }

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
            material: Material::default(),
            glass: false,
            activation: Activation::default(),
            backend: Backend::disconnected(),
            apps: vec![app],
            usage: Usage::disabled(),
            favorites: Favorites::disabled(),
            opening: None,
            launch_task: None,
            unconfirmed_open: false,
            clipboard_owner: false,
            icons: Icons::disabled(),
            query: String::new(),
            scope: Scope::All,
            search_folder: None,
            folder_more: false,
            folder_search_history: Vec::new(),
            scope_generation: 0,
            app_hits: Vec::new(),
            files: Vec::new(),
            items: Vec::new(),
            item_icons: Vec::new(),
            cursor: Cursor::new(VISIBLE_ROWS),
            clicks: ClickTracker::default(),
            panel: None,
            search_status: None,
            application_picker: None,
            application_generation: 0,
            file_details: None,
            details_generation: 0,
            preview_enabled: false,
            window_width: WIDTH,
            text_preview: None,
            preview_generation: 0,
            preview_request: None,
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
        assert_eq!(panel.actions.len(), 9);
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
    fn open_with_filters_choices_and_dispatches_the_captured_file() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let search_generation = s.generation;
        let files = file_hits();
        let captured = files[0].clone();
        let _ = update(&mut s, Message::Files { generation: search_generation, result: Ok(Some(files.clone())) });
        let _ = update(&mut s, Message::Selected(Target::File(captured.clone())));
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("open with".into()));
        let _ = command(&mut s, Command::Activate);
        assert!(s.application_picker.is_some());
        assert!(s.opening.is_none());
        let generation = s.application_generation;
        let _ = update(
            &mut s,
            Message::ApplicationsLoaded {
                generation,
                result: Ok(vec![
                    crate::backend::ApplicationChoice {
                        id: "one.desktop".into(),
                        name: "Editor One".into(),
                        icon: String::new(),
                    },
                    crate::backend::ApplicationChoice {
                        id: "two.desktop".into(),
                        name: "Editor Two".into(),
                        icon: String::new(),
                    },
                ]),
            },
        );
        let _ = update(&mut s, Message::Selected(Target::File(files[1].clone())));
        let _ = update(&mut s, Message::Query("offscreen input".into()));
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::Files);
        let _ = update(&mut s, Message::OpenWith { generation, event: crate::open_with::Event::Query("two".into()) });
        let _ = command(&mut s, Command::Activate);
        assert_eq!(s.opening.as_ref(), Some(&(captured.id.clone(), "report".into())));
        assert_eq!(s.usage.boost(Kind::File, &captured.id, "report", usage::now()), 0);
        let _ = update(&mut s, Message::Opened(Ok(())));
        assert!(s.usage.boost(Kind::File, &captured.id, "report", usage::now()) > 0);
    }

    #[test]
    fn file_details_action_is_discoverable_and_preserves_search_on_return() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let target = s.selected().unwrap().target.clone();
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("properties".into()));
        let actions = s.panel.as_ref().unwrap().filtered_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].label(), "File details");
        let _ = command(&mut s, Command::Activate);
        let _ = update(&mut s, Message::Query("queued input".into()));
        let _ = update(&mut s, Message::Scope(Scope::All));
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::Files);
        let _ = command(&mut s, Command::Back);
        assert_eq!(s.selected().unwrap().target, target);
        assert_eq!(s.query, "report");
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn details_ignore_stale_replies_and_restore_the_captured_file() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let search_generation = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::Files { generation: search_generation, result: Ok(Some(files.clone())) });
        let captured = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::Details);
        let old = s.details_generation;
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.details_generation, old, "one read at a time");
        let _ = update(&mut s, Message::Selected(Target::File(files[1].clone())));
        let _ = update(&mut s, Message::ShowFolders);
        let _ = update(&mut s, Message::ShowSearchStatus);
        let _ = command(&mut s, Command::ActivateVisible(0));
        assert!(s.opening.is_none());
        assert!(s.folder_settings.is_none() && s.search_status.is_none());
        let _ = update(&mut s, Message::DetailsLoaded { generation: old, result: Err("unavailable".into()) });
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let refreshing = s.details_generation;
        assert!(refreshing > old);
        let _ = update(&mut s, Message::DetailsLoaded { generation: old, result: Err("late error".into()) });
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.details_generation, refreshing, "late replies must leave the active read loading");
        let _ = command(&mut s, Command::Back);
        let _ = s.perform(Action::Details);
        let current = s.details_generation;
        let _ = update(&mut s, Message::Details { generation: old, event: details::Event::Back });
        let _ = update(&mut s, Message::DetailsLoaded { generation: refreshing, result: Err("closed".into()) });
        assert!(s.file_details.is_some());
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.details_generation, current);
        let _ = update(
            &mut s,
            Message::DetailsLoaded {
                generation: current,
                result: Ok(crate::backend::FileDetails { mime: String::new(), size: 0, modified: 0 }),
            },
        );
        let _ = update(
            &mut s,
            Message::Files {
                generation: search_generation,
                result: Ok(Some(vec![files[1].clone(), files[0].clone()])),
            },
        );
        assert!(matches!(s.file_details.as_mut().unwrap().event(details::Event::CopyLocation),
            details::Effect::Copy(path) if path == "/synthetic/report.txt"));
        let _ = command(&mut s, Command::Back);
        assert!(s.file_details.is_none());
        assert_eq!(s.selected().unwrap().target, captured);
        assert_eq!(s.scope, Scope::Files);
        assert_eq!(s.query, "report");
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn details_widget_and_subscription_route_keys_to_the_modal_view() {
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        use iced::keyboard::key::{Code, Physical};
        use iced::{Rectangle, Size};

        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let captured = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::Details);
        let initial = s.details_generation;
        let loaded = |generation| Message::DetailsLoaded {
            generation,
            result: Ok(crate::backend::FileDetails { mime: "application/pdf".into(), size: 4096, modified: -1 }),
        };
        let _ = update(&mut s, loaded(initial));
        for (key, code, modifiers) in [
            (Key::Named(keyboard::key::Named::ArrowDown), Code::ArrowDown, Modifiers::empty()),
            (Key::Character("r".into()), Code::KeyR, Modifiers::CTRL),
            (Key::Character("C".into()), Code::KeyC, Modifiers::CTRL | Modifiers::SHIFT),
            (Key::Named(keyboard::key::Named::Escape), Code::Escape, Modifiers::empty()),
        ] {
            let event = Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key: key.clone(),
                key,
                physical_key: Physical::Code(code),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            });
            let mut messages = {
                let renderer =
                    iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
                let mut element = view(&s);
                let widget = element.as_widget_mut();
                let mut tree = Tree::new(&*widget);
                let bounds = Size::new(720.0, 500.0);
                let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                widget.update(
                    &mut tree,
                    &event,
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                    messages.push(message);
                }
                messages
            };
            assert_eq!(messages.len(), 1, "one route per essential keyboard event");
            for message in messages.drain(..) {
                let _ = update(&mut s, message);
            }
            if code == Code::KeyR {
                assert!(s.details_generation > initial);
                let generation = s.details_generation;
                let _ = update(&mut s, loaded(generation));
            }
            assert_eq!(s.query, "report");
            assert_eq!(s.scope, Scope::Files);
            assert!(s.opening.is_none());
            assert!(s.usage.recent(usage::now(), 8).is_empty());
        }
        assert!(s.file_details.is_none());
        assert_eq!(s.selected().unwrap().target, captured);
    }

    #[test]
    fn preview_widget_and_subscription_keep_search_navigation_and_sidebar_scrolling() {
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        use iced::keyboard::key::{Code, Physical};
        use iced::{Rectangle, Size};

        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let captured = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::Preview);
        let initial = s.preview_generation;
        let loaded = |generation| Message::PreviewLoaded { generation, result: preview_data("synthetic text") };
        let _ = update(&mut s, loaded(initial));
        for (key, code, modifiers) in [
            (Key::Named(keyboard::key::Named::PageDown), Code::PageDown, Modifiers::ALT),
            (Key::Character("r".into()), Code::KeyR, Modifiers::CTRL),
            (Key::Character("C".into()), Code::KeyC, Modifiers::CTRL | Modifiers::SHIFT),
            (Key::Named(keyboard::key::Named::Escape), Code::Escape, Modifiers::empty()),
        ] {
            let event = Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key: key.clone(),
                key,
                physical_key: Physical::Code(code),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            });
            let mut messages = {
                let renderer =
                    iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
                let mut element = view(&s);
                let widget = element.as_widget_mut();
                let mut tree = Tree::new(&*widget);
                let bounds = Size::new(PREVIEW_WIDTH as f32, full_height() as f32);
                let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
                widget.operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()),
                );
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                widget.update(
                    &mut tree,
                    &event,
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                    messages.push(message);
                }
                messages
            };
            assert_eq!(messages.len(), 1, "one route per essential keyboard event");
            for message in messages.drain(..) {
                let _ = update(&mut s, message);
            }
            if code == Code::KeyR {
                assert!(s.preview_generation > initial);
                let generation = s.preview_generation;
                let _ = update(&mut s, loaded(generation));
            }
            assert_eq!(s.query, "report");
            assert_eq!(s.scope, Scope::Files);
            assert!(s.opening.is_none());
            assert!(s.usage.recent(usage::now(), 8).is_empty());
        }
        assert!(s.text_preview.is_none());
        assert_eq!(s.selected().unwrap().target, captured);
    }

    #[test]
    fn details_copy_non_utf8_locations_as_lossless_file_urls() {
        use std::os::unix::ffi::OsStringExt;
        let mut file = file_hits().remove(0);
        file.id = "file:///synthetic/non%FF.pdf".into();
        file.path = Some(std::ffi::OsString::from_vec(b"/synthetic/non\xff.pdf".to_vec()).into());
        let mut panel = details::Panel::new(file.clone());
        assert!(matches!(panel.command(Command::CopyPath), details::Effect::Copy(url) if url == file.id));
    }

    #[test]
    fn search_in_folder_action_preserves_the_original_query_scope_and_target() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let mut folder = file_hits().remove(0);
        folder.id = "file:///synthetic/Reports".into();
        folder.path = Some("/synthetic/Reports".into());
        folder.name = "Reports".into();
        folder.is_dir = true;
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![folder.clone()])) });
        let target = Target::File(folder);
        s.cursor.select_target(&target, &s.items);
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("within".into()));
        let actions = s.panel.as_ref().unwrap().filtered_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].label(), "Browse folder");
        let _ = command(&mut s, Command::Activate);
        assert_eq!(s.scope, Scope::Files);
        assert!(s.query.is_empty());
        assert!(s.items.is_empty(), "global suggestions must not appear inside a folder");
        let _ = update(&mut s, Message::Query("nested".into()));
        let _ = command(&mut s, Command::Back);
        assert!(s.query.is_empty());
        let _ = command(&mut s, Command::Back);
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::All);
        assert_eq!(s.selected().unwrap().target, target);
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    fn search_folder_hit(path: &str) -> FileHit {
        FileHit {
            id: format!("file://{path}"),
            path: Some(path.into()),
            name: std::path::Path::new(path).file_name().unwrap().to_string_lossy().into(),
            icon: "folder".into(),
            subtext: "/synthetic".into(),
            is_dir: true,
        }
    }

    #[test]
    fn nested_folder_search_rejects_stale_replies_and_back_buttons() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let root_generation = s.generation;
        let root_input = s.scope_generation;
        let parent = search_folder_hit("/synthetic/Reports");
        let _ = update(&mut s, Message::Files { generation: root_generation, result: Ok(Some(vec![parent.clone()])) });
        let root_target = Target::File(parent.clone());
        s.cursor.select_target(&root_target, &s.items);
        let _ = command(&mut s, Command::BrowseFolder);
        let parent_back = s.scope_generation;
        let _ = update(&mut s, Message::ContextQuery { generation: root_input, query: "late global input".into() });
        assert!(s.query.is_empty());
        let _ = update(&mut s, Message::Files { generation: root_generation, result: Ok(Some(file_hits())) });
        assert!(s.items.is_empty());
        let _ = update(&mut s, Message::Scope(Scope::All));
        let _ = command(&mut s, Command::NextScope);
        assert_eq!(s.scope, Scope::Files);
        let _ = update(&mut s, Message::Query("=2+3".into()));
        assert!(s.items.is_empty(), "folder searches cannot become calculator queries");
        let parent_query = s.generation;
        let child = search_folder_hit("/synthetic/Reports/Child");
        let _ = update(&mut s, Message::Files { generation: parent_query, result: Ok(Some(vec![child.clone()])) });
        let _ = command(&mut s, Command::BrowseFolder);
        assert_eq!(s.search_folder.as_ref(), Some(&child));
        let _ = update(&mut s, Message::FolderBack { generation: parent_back });
        assert_eq!(s.search_folder.as_ref(), Some(&child));
        let _ = update(&mut s, Message::Files { generation: parent_query, result: Ok(Some(file_hits())) });
        assert!(s.items.is_empty());
        let _ = update(&mut s, Message::Query("child".into()));
        let old = s.generation;
        let _ = update(&mut s, Message::Files { generation: old, result: Err("unavailable".into()) });
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let pending_child = s.generation;
        assert!(pending_child > old);
        let _ = update(&mut s, Message::Files { generation: old, result: Err("late".into()) });
        assert_eq!(s.status, Status::Searching);
        let _ = command(&mut s, Command::BackFolder);
        assert_eq!(s.search_folder.as_ref(), Some(&parent));
        assert_eq!(s.scope, Scope::Files);
        let _ = update(
            &mut s,
            Message::ContextQuery { generation: parent_back, query: "input from departed parent view".into() },
        );
        assert_eq!(s.query, "=2+3");
        assert_eq!(s.selected().unwrap().target, Target::File(child));
        let _ = update(&mut s, Message::Files { generation: pending_child, result: Ok(Some(file_hits())) });
        assert_eq!(s.items.len(), 1);
        let _ = command(&mut s, Command::BackFolder);
        assert!(s.search_folder.is_none());
        assert_eq!(s.scope, Scope::All);
        assert_eq!(s.query, "report");
        assert_eq!(s.selected().unwrap().target, root_target);
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn applying_folder_configuration_restores_the_original_global_query() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(
            &mut s,
            Message::Files { generation, result: Ok(Some(vec![search_folder_hit("/synthetic/Reports")])) },
        );
        s.cursor.select_target(&Target::File(search_folder_hit("/synthetic/Reports")), &s.items);
        let _ = command(&mut s, Command::BrowseFolder);
        let _ = update(&mut s, Message::Query("local query".into()));
        let stale = s.generation;
        let _ = s.finish_folders(Task::none(), folders::Effect::Applied);
        assert!(s.search_folder.is_none() && s.folder_search_history.is_empty());
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::All);
        let _ = update(&mut s, Message::Files { generation: stale, result: Ok(Some(file_hits())) });
        assert!(s.files.is_empty());
    }

    #[test]
    fn empty_folder_queries_do_not_show_global_recent_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("global-recent.txt");
        std::fs::write(&file, "fixture").unwrap();
        let mut s = search_state();
        s.usage.record(Kind::File, &format!("file://{}", file.display()), "", usage::now());
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query(String::new()));
        assert!(!s.items.is_empty());
        let _ = s.enter_folder_search(search_folder_hit("/synthetic/Reports"));
        assert!(s.items.is_empty());
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(Vec::new())) });
        assert!(s.items.is_empty());
    }

    #[test]
    fn folder_search_keys_pass_through_the_focused_widget_and_subscription() {
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        use iced::keyboard::key::{Code, Physical};
        use iced::{Rectangle, Size};
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let folder = search_folder_hit("/synthetic/Reports");
        let target = Target::File(folder.clone());
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![folder])) });
        s.cursor.select_target(&target, &s.items);
        let mut departed_root_input = None;
        let mut departed_child_input = None;
        for (key, code, modifiers, typed) in [
            (Key::Character("x".into()), Code::KeyX, Modifiers::empty(), Some("x")),
            (Key::Named(keyboard::key::Named::ArrowRight), Code::ArrowRight, Modifiers::CTRL, None),
            (Key::Named(keyboard::key::Named::ArrowRight), Code::ArrowRight, Modifiers::ALT, None),
            (Key::Character("n".into()), Code::KeyN, Modifiers::empty(), Some("n")),
            (Key::Character("x".into()), Code::KeyX, Modifiers::empty(), Some("x")),
            (Key::Character("r".into()), Code::KeyR, Modifiers::CTRL, None),
            (Key::Named(keyboard::key::Named::ArrowLeft), Code::ArrowLeft, Modifiers::ALT, None),
        ] {
            let event = Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key: key.clone(),
                key,
                physical_key: Physical::Code(code),
                location: keyboard::Location::Standard,
                modifiers,
                text: typed.map(Into::into),
                repeat: false,
            });
            let messages = {
                let renderer =
                    iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
                let mut element = view(&s);
                let widget = element.as_widget_mut();
                let mut tree = Tree::new(&*widget);
                let bounds = Size::new(720.0, 500.0);
                let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
                let layout = Layout::new(&node);
                widget.operate(
                    &mut tree,
                    layout,
                    &renderer,
                    &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()),
                );
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                widget.update(
                    &mut tree,
                    &Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
                    layout,
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                widget.update(
                    &mut tree,
                    &event,
                    layout,
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                    messages.push(message);
                }
                messages
            };
            if code == Code::ArrowRight && modifiers == Modifiers::CTRL {
                assert!(messages.is_empty(), "word navigation must not enter a folder");
                assert!(s.search_folder.is_none());
                assert_eq!(s.query, "report");
                continue;
            }
            assert_eq!(messages.len(), 1);
            if code == Code::KeyX {
                if s.search_folder.is_none() {
                    departed_root_input = messages.into_iter().next();
                } else {
                    departed_child_input = messages.into_iter().next();
                }
                continue;
            }
            for message in messages {
                let _ = update(&mut s, message);
            }
            if code == Code::ArrowRight {
                let _ = update(&mut s, departed_root_input.take().unwrap());
                assert!(s.query.is_empty() && s.search_folder.is_some());
            }
            if code == Code::ArrowLeft {
                let _ = update(&mut s, departed_child_input.take().unwrap());
            }
            if code == Code::KeyN || code == Code::KeyR {
                assert_eq!(s.query, "n");
            }
            assert!(s.opening.is_none());
        }
        assert!(s.search_folder.is_none());
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::All);
        assert_eq!(s.selected().unwrap().target, target);
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn preview_action_preserves_search_and_allows_editing_without_recording_a_launch() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        assert_eq!(s.preview_generation, 0);
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("preview".into()));
        assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::Preview]);
        let _ = command(&mut s, Command::Activate);
        assert!(s.preview_enabled && s.preview_visible());
        assert_eq!(s.window_width, PREVIEW_WIDTH);
        assert_eq!(s.query, "report");
        let _ = update(&mut s, Message::Query("quarterly".into()));
        let _ = update(&mut s, Message::Scope(Scope::All));
        assert_eq!(s.query, "quarterly");
        assert_eq!(s.scope, Scope::All);
        assert!(s.text_preview.is_none(), "a new query clears the old text immediately");
        let _ = command(&mut s, Command::Back);
        assert!(!s.preview_enabled);
        assert_eq!(s.query, "quarterly", "Esc hides the sidebar before clearing the query");
        assert_eq!(s.window_width, WIDTH);
        assert!(s.opening.is_none());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn preview_follows_selection_and_rejects_stale_content_and_departed_widgets() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let search = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::Files { generation: search, result: Ok(Some(files.clone())) });
        let _ = s.perform(Action::Preview);
        let old = s.preview_generation;
        let _ = command(&mut s, Command::Down);
        let current = s.preview_generation;
        assert!(current > old);
        assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
        for message in [
            Message::PreviewLoaded { generation: old, result: Err("late old file".into()) },
            Message::Preview { generation: old, event: preview::Event::Back },
            Message::Preview { generation: old, event: preview::Event::Refresh },
        ] {
            let _ = update(&mut s, message);
        }
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.preview_generation, current, "stale replies leave the current read loading");
        let _ = update(&mut s, Message::PreviewLoaded { generation: current, result: Err("unavailable".into()) });
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let refreshing = s.preview_generation;
        assert!(refreshing > current);
        let _ = update(
            &mut s,
            Message::Files { generation: search, result: Ok(Some(vec![files[1].clone(), files[0].clone()])) },
        );
        assert_eq!(s.preview_generation, refreshing, "a stable target does not trigger another read");
        assert!(
            matches!(s.text_preview.as_mut().unwrap().event(preview::Event::CopyLocation), preview::Effect::Copy(path) if path == "/synthetic/report.pdf")
        );
        let _ = command(&mut s, Command::Back);
        let stopped = s.preview_generation;
        let _ = command(&mut s, Command::Down);
        let _ = update(&mut s, Message::PreviewLoaded { generation: refreshing, result: preview_data("closed") });
        assert!(!s.preview_enabled && s.text_preview.is_none());
        assert_eq!(s.preview_generation, stopped, "disabled preview never reads on navigation");
        assert_eq!(s.query, "report");
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn preview_from_details_returns_to_results_with_the_captured_file_selected() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let target = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::Details);
        let details = s.details_generation;
        let _ = command(&mut s, Command::Activate);
        assert!(s.preview_visible() && s.file_details.is_none());
        assert_eq!(s.selected().unwrap().target, target);
        let _ = update(&mut s, Message::Details { generation: details, event: details::Event::Back });
        assert!(s.preview_visible(), "the departed details button cannot hide the sidebar");
        let _ = command(&mut s, Command::Back);
        assert!(!s.preview_enabled && s.file_details.is_none());
        assert_eq!(s.query, "report");
        assert!(s.opening.is_none());
    }

    #[test]
    fn preview_keeps_normal_open_actions_and_pauses_reads_behind_modal_screens() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
        let _ = s.perform(Action::Preview);
        let read = s.preview_generation;
        let _ = update(&mut s, Message::PreviewLoaded { generation: read, result: preview_data("ready") });
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert!(!s.preview_visible());
        let paused = s.preview_generation;
        let _ = update(&mut s, Message::ResultsWheel(-1.0));
        assert_eq!(s.preview_generation, paused);
        let _ = update(&mut s, Message::CloseSearchStatus);
        assert!(s.preview_visible());
        assert_eq!(s.preview_generation, paused, "returning to the same target reuses its completed data");
        let _ = update(&mut s, Message::Selected(Target::File(files[1].clone())));
        assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
        let _ = command(&mut s, Command::Activate);
        assert_eq!(s.opening.as_ref().unwrap().0, files[1].id);
        assert!(s.usage.recent(usage::now(), 8).is_empty(), "only confirmed launches record history");
    }

    #[test]
    fn an_explicit_preview_follows_keyboard_selection_without_leaving_the_results() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
        assert!(s.text_preview.is_none());
        let _ = s.perform(Action::Preview);
        let old = s.preview_generation;
        let _ = command(&mut s, Command::Down);
        assert_eq!(s.selected().unwrap().target, Target::File(files[1].clone()));
        assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
        assert!(s.preview_generation > old);
        assert_eq!(s.query, "report");
        assert!(s.opening.is_none());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    fn pointer_messages(s: &Launcher, point: iced::Point, events: Vec<mouse::Event>) -> Vec<Message> {
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        use iced::{Rectangle, Size};
        let renderer =
            iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
        let mut element = view(s);
        let widget = element.as_widget_mut();
        let mut tree = Tree::new(&*widget);
        let bounds = Size::new(s.window_width as f32, full_height() as f32);
        let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
        let mut messages = Vec::new();
        for event in events {
            let event = Event::Mouse(event);
            let status = {
                let mut shell = Shell::new(&mut messages);
                widget.update(
                    &mut tree,
                    &event,
                    Layout::new(&node),
                    mouse::Cursor::Available(point),
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                shell.event_status()
            };
            if let Some(message) = on_event(event, status, iced::window::Id::unique()) {
                messages.push(message);
            }
        }
        messages
    }

    #[test]
    fn sidebar_pointer_and_wheel_events_keep_text_and_result_navigation_separate() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
        let _ = s.perform(Action::Preview);
        let read = s.preview_generation;
        let _ = update(
            &mut s,
            Message::PreviewLoaded { generation: read, result: preview_data(&"synthetic line\n".repeat(100)) },
        );
        let wheel = || mouse::Event::WheelScrolled { delta: ScrollDelta::Lines { x: 0.0, y: -1.0 } };
        let messages = pointer_messages(&s, iced::Point::new(650.0, 200.0), vec![wheel()]);
        assert!(messages.is_empty(), "the text scrollable captures its wheel without changing selection");
        assert_eq!(s.selected().unwrap().target, Target::File(files[0].clone()));
        let messages = pointer_messages(&s, iced::Point::new(120.0, 100.0), vec![wheel()]);
        assert!(matches!(&messages[..], [Message::ResultsWheel(y)] if *y == -1.0), "one route per list wheel event");
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert_eq!(s.selected().unwrap().target, Target::File(files[1].clone()));
        assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
        let messages = pointer_messages(
            &s,
            iced::Point::new(120.0, 95.0),
            vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
        );
        assert!(messages.iter().any(|message| matches!(message, Message::Selected(_))));
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert_eq!(s.selected().unwrap().target, Target::File(files[0].clone()));
        assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[0].id);
        assert!(s.opening.is_none(), "one click selects without opening");
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn hiding_or_disabling_preview_invalidates_pending_bus_requests() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let _ = s.perform(Action::Preview);
        let first = s.preview_request.clone().unwrap();
        let _ = command(&mut s, Command::Down);
        assert!(!first.is_active(), "old queued selections cannot start later");
        let second = s.preview_request.clone().unwrap();
        let old = s.preview_generation;
        let _ = update(&mut s, Message::ShowSearchStatus);
        assert!(!second.is_active());
        assert!(s.text_preview.is_none());
        let _ = update(&mut s, Message::PreviewLoaded { generation: old, result: preview_data("hidden") });
        assert!(s.text_preview.is_none());
        let _ = update(&mut s, Message::CloseSearchStatus);
        let resumed = s.preview_request.clone().unwrap();
        assert!(resumed.is_active());
        let _ = command(&mut s, Command::Back);
        assert!(!resumed.is_active());
        assert!(!s.preview_enabled && s.text_preview.is_none());
    }

    #[test]
    fn a_new_query_invalidates_text_even_when_home_selects_the_same_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.txt");
        std::fs::write(&path, "synthetic contents").unwrap();
        let file = FileHit {
            id: format!("file://{}", path.display()),
            name: "report.txt".into(),
            path: Some(path),
            icon: String::new(),
            subtext: String::new(),
            is_dir: false,
        };
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![file.clone()])) });
        let _ = s.perform(Action::AddFavorite);
        let _ = s.perform(Action::Preview);
        let old = s.preview_generation;
        let _ = update(&mut s, Message::PreviewLoaded { generation: old, result: preview_data("old text") });
        let _ = update(&mut s, Message::Query(String::new()));
        assert!(matches!(&s.selected().unwrap().target, Target::File(selected) if selected.id == file.id));
        let fresh = s.preview_generation;
        assert!(fresh > old);
        assert!(s.preview_request.as_ref().unwrap().is_active());
        let _ = update(&mut s, Message::PreviewLoaded { generation: old, result: preview_data("stale") });
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.preview_generation, fresh, "stale content cannot replace the new loading state");
    }

    #[test]
    fn sidebar_toolbar_buttons_enter_refresh_copy_and_hide_through_real_widgets() {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let _ = s.perform(Action::Details);
        let click = |s: &Launcher, x, y| {
            pointer_messages(
                s,
                iced::Point::new(x, y),
                vec![
                    mouse::Event::ButtonPressed(mouse::Button::Left),
                    mouse::Event::ButtonReleased(mouse::Button::Left),
                ],
            )
        };
        let messages = click(&s, 500.0, 35.0);
        assert!(matches!(&messages[..], [Message::Details { event: details::Event::Preview, .. }]));
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert!(s.preview_visible());
        assert!(click(&s, 810.0, 90.0).is_empty(), "refresh is disabled while loading");
        let initial = s.preview_generation;
        let _ = update(&mut s, Message::PreviewLoaded { generation: initial, result: preview_data("synthetic text") });
        let messages = click(&s, 920.0, 350.0);
        assert!(matches!(&messages[..], [Message::Preview { event: preview::Event::CopyLocation, .. }]));
        for message in messages {
            let _ = update(&mut s, message);
        }
        let messages = click(&s, 810.0, 90.0);
        assert!(matches!(&messages[..], [Message::Preview { event: preview::Event::Refresh, .. }]));
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert!(s.preview_generation > initial);
        let pending = s.preview_request.clone().unwrap();
        let messages = click(&s, 920.0, 90.0);
        assert!(matches!(&messages[..], [Message::Preview { event: preview::Event::Back, .. }]));
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert!(!s.preview_enabled && !pending.is_active());
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::Files);
        assert!(s.opening.is_none());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    fn browser_state() -> (Launcher, FileHit) {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let mut folder = file_hits().remove(0);
        folder.id = "file:///synthetic/Reports".into();
        folder.path = Some("/synthetic/Reports".into());
        folder.name = "Reports".into();
        folder.is_dir = true;
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![folder.clone()])) });
        let _ = command(&mut s, Command::BrowseFolder);
        (s, folder)
    }

    #[test]
    fn browsing_rejects_old_lists_and_restores_parent_selection_and_limit_state() {
        let (mut s, folder) = browser_state();
        let generation = s.generation;
        let mut child = folder.clone();
        child.id.push_str("/Nested");
        child.path = Some("/synthetic/Reports/Nested".into());
        child.name = "Nested".into();
        let file = file_hits().remove(0);
        let children = vec![child.clone(), file.clone()];
        let _ = update(&mut s, Message::FolderListed { generation, result: Ok((children.clone(), true)) });
        assert!(s.folder_more);
        assert_eq!(s.selected().unwrap().target, Target::File(child.clone()));
        assert_eq!(s.preview_generation, 0, "listing alone does not read file content");
        let _ = command(&mut s, Command::BrowseFolder);
        let nested = s.generation;
        let _ = update(&mut s, Message::FolderListed { generation: nested, result: Ok((vec![file], false)) });
        let _ = command(&mut s, Command::BackFolder);
        assert_eq!(s.search_folder.as_ref().unwrap().id, folder.id);
        assert_eq!(s.selected().unwrap().target, Target::File(child));
        assert!(s.folder_more);
        let _ = update(&mut s, Message::FolderListed { generation: nested, result: Ok((Vec::new(), false)) });
        assert_eq!(s.items.len(), 2);
        assert!(s.folder_more);
        let _ = command(&mut s, Command::BackFolder);
        assert!(s.search_folder.is_none());
        assert_eq!(s.query, "report");
        assert!(!s.folder_more);
        assert_eq!(s.selected().unwrap().target, Target::File(folder));
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    #[test]
    fn listing_and_recursive_queries_cannot_replace_each_others_results() {
        let (mut s, _) = browser_state();
        let listing = s.generation;
        let _ = update(&mut s, Message::Query("report".into()));
        let recursive = s.generation;
        let _ = update(&mut s, Message::FolderListed { generation: listing, result: Ok((file_hits(), true)) });
        assert!(s.items.is_empty() && !s.folder_more);
        let _ = update(&mut s, Message::Files { generation: recursive, result: Ok(Some(file_hits())) });
        assert_eq!(s.items.len(), 2);
        let _ = update(&mut s, Message::Query(String::new()));
        let current = s.generation;
        let _ = update(&mut s, Message::Files { generation: recursive, result: Ok(Some(file_hits())) });
        assert!(s.items.is_empty());
        let _ = update(&mut s, Message::FolderListed { generation: current, result: Ok((Vec::new(), false)) });
        assert!(s.items.is_empty() && matches!(s.status, Status::Idle));
        assert!(s.search_folder.is_some(), "an empty folder does not fall back to global suggestions");
    }

    #[test]
    fn folder_queries_wait_for_two_characters_before_searching() {
        let (mut s, _) = browser_state();
        for query in ["a", " é ", "日", "🦀"] {
            let _ = update(&mut s, Message::Query("report".into()));
            let previous = s.generation;
            let _ = update(&mut s, Message::Files { generation: previous, result: Ok(Some(file_hits())) });
            assert!(!s.items.is_empty());

            let _ = update(&mut s, Message::Query(query.into()));
            assert_eq!(s.status, Status::Idle, "short query: {query:?}");
            assert!(s.files.is_empty() && s.items.is_empty());
            let current = s.generation;
            let _ = update(&mut s, Message::RefreshSearchStatus);
            assert_eq!(s.generation, current, "refresh must not start a short search");
            assert_eq!(s.status, Status::Idle);

            let _ = update(&mut s, Message::Files { generation: previous, result: Ok(Some(file_hits())) });
            let _ = update(&mut s, Message::FolderListed { generation: previous, result: Ok((file_hits(), true)) });
            assert!(s.items.is_empty() && !s.folder_more);
            assert_eq!(s.status, Status::Idle);
        }

        for query in ["ré", " 日本 ", "=x"] {
            let _ = update(&mut s, Message::Query(query.into()));
            assert_eq!(s.status, Status::Searching, "searchable query: {query:?}");
            let generation = s.generation;
            let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
            assert_eq!(s.items.len(), 2);
        }

        for query in ["", " \t "] {
            let _ = update(&mut s, Message::Query(query.into()));
            assert_eq!(s.status, Status::Searching);
            let generation = s.generation;
            let _ = update(&mut s, Message::FolderListed { generation, result: Ok((file_hits(), true)) });
            assert_eq!(s.items.len(), 2);
            assert!(s.folder_more && s.search_folder.is_some());
        }
    }

    #[test]
    fn folder_refresh_preserves_selection_and_reloads_an_enabled_preview() {
        let (mut s, _) = browser_state();
        let generation = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::FolderListed { generation, result: Ok((files.clone(), false)) });
        let _ = command(&mut s, Command::Down);
        let target = s.selected().unwrap().target.clone();
        let _ = s.perform(Action::Preview);
        let pending = s.preview_request.clone().unwrap();
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert!(!pending.is_active());
        assert_eq!(s.selected().unwrap().target, target);
        let refresh = s.generation;
        let _ = update(
            &mut s,
            Message::FolderListed {
                generation: refresh,
                result: Ok((vec![files[1].clone(), files[0].clone()], false)),
            },
        );
        assert_eq!(s.selected().unwrap().target, target);
        assert!(s.preview_request.as_ref().unwrap().is_active());
        assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
    }

    #[test]
    fn folder_back_button_restores_the_child_and_rejects_a_departed_header_click() {
        let (mut s, folder) = browser_state();
        let mut child = folder.clone();
        child.id.push_str("/Nested");
        child.path = Some("/synthetic/Reports/Nested".into());
        child.name = "Nested".into();
        let generation = s.generation;
        let _ = update(&mut s, Message::FolderListed { generation, result: Ok((vec![child.clone()], false)) });
        let _ = command(&mut s, Command::BrowseFolder);
        let generation = s.generation;
        let _ = update(&mut s, Message::FolderListed { generation, result: Ok((Vec::new(), false)) });
        let departed = s.scope_generation;
        let messages = pointer_messages(
            &s,
            iced::Point::new(70.0, 22.0),
            vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
        );
        assert!(matches!(&messages[..], [Message::FolderBack { .. }]));
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert_eq!(s.search_folder.as_ref().unwrap().id, folder.id);
        assert_eq!(s.selected().unwrap().target, Target::File(child.clone()));
        let refresh = s.generation;
        let _ = update(&mut s, Message::FolderListed { generation: refresh, result: Ok((vec![child.clone()], false)) });
        let _ = update(&mut s, Message::FolderBack { generation: departed });
        assert_eq!(s.search_folder.as_ref().unwrap().id, folder.id);
        assert_eq!(s.selected().unwrap().target, Target::File(child));
        assert!(s.query.is_empty());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }

    fn preview_data(text: &str) -> Result<crate::backend::PreviewData, String> {
        Ok(crate::backend::PreviewData {
            content: Ok(crate::backend::TextPreview::Text { text: text.into(), truncated: false }),
            details: Ok(crate::backend::FileDetails {
                mime: "text/plain".into(),
                size: text.len() as u64,
                modified: 0,
            }),
        })
    }

    fn picker_state() -> Launcher {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let _ = s.perform(Action::OpenWith);
        s
    }

    fn picker_choices(count: usize) -> Vec<crate::backend::ApplicationChoice> {
        (0..count)
            .map(|i| crate::backend::ApplicationChoice {
                id: format!("editor-{i:02}.desktop"),
                name: format!("Editor {i:02}"),
                icon: String::new(),
            })
            .collect()
    }

    #[test]
    fn picker_escape_restores_search_and_ignores_closed_or_reopened_generations() {
        let mut s = picker_state();
        let old = s.application_generation;
        let target = s.selected().unwrap().target.clone();
        let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Ok(picker_choices(12)) });
        let _ = update(&mut s, Message::OpenWith { generation: old, event: open_with::Event::Query("07".into()) });
        let _ = command(&mut s, Command::Back);
        assert!(s.application_picker.as_ref().unwrap().query.is_empty());
        let _ = command(&mut s, Command::Back);
        assert!(s.application_picker.is_none());
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::Files);
        assert_eq!(s.selected().unwrap().target, target);
        let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Err("late".into()) });
        assert!(s.application_picker.is_none());
        let _ = s.perform(Action::OpenWith);
        let current = s.application_generation;
        let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Ok(picker_choices(12)) });
        let _ = update(
            &mut s,
            Message::OpenWith { generation: old, event: open_with::Event::Open("editor-00.desktop".into()) },
        );
        assert!(s.opening.is_none());
        let _ = update(&mut s, Message::ApplicationsLoaded { generation: current, result: Ok(picker_choices(12)) });
        let _ = update(&mut s, Message::OpenWith { generation: current, event: open_with::Event::Query("07".into()) });
        let _ = update(
            &mut s,
            Message::OpenWith { generation: current, event: open_with::Event::Open("editor-00.desktop".into()) },
        );
        assert!(s.opening.is_none(), "a filtered-out pointer choice must not dispatch");
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_some());
    }

    #[test]
    fn picker_read_errors_can_retry_but_unconfirmed_dispatch_cannot_duplicate() {
        let mut s = picker_state();
        let old = s.application_generation;
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.application_generation, old, "one list request at a time");
        let _ =
            update(&mut s, Message::ApplicationsLoaded { generation: old, result: Err("service unavailable".into()) });
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_none());
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let generation = s.application_generation;
        assert!(generation > old);
        let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Ok(picker_choices(2)) });
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_none());
        let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(Vec::new()) });
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_none());
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let generation = s.application_generation;
        let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(2)) });
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_some());
        let _ = update(&mut s, Message::Opened(Err(RunError::Unconfirmed("connection lost".into()))));
        assert!(s.unconfirmed_open);
        assert!(s.usage.recent(usage::now(), 8).is_empty());
        let _ = command(&mut s, Command::Activate);
        assert!(s.opening.is_none());
        let _ = command(&mut s, Command::Back);
        let _ = s.perform(Action::OpenWith);
        assert!(s.application_picker.is_none(), "close and reopen the process before retrying an uncertain launch");
    }

    #[test]
    fn picker_rejected_dispatch_can_choose_another_application() {
        let mut s = picker_state();
        let generation = s.application_generation;
        let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(2)) });
        let _ = command(&mut s, Command::Activate);
        let (file, query) = s.opening.clone().unwrap();
        let _ = update(&mut s, Message::Opened(Err(RunError::Failed("executable is unavailable".into()))));
        assert!(!s.unconfirmed_open);
        assert!(s.opening.is_none());
        assert_eq!(s.usage.boost(Kind::File, &file, &query, usage::now()), 0);
        let _ = update(&mut s, Message::OpenWith { generation, event: open_with::Event::Query("01".into()) });
        let _ = command(&mut s, Command::Activate);
        assert_eq!(s.opening.as_ref(), Some(&(file.clone(), query.clone())));
        assert_eq!(s.application_picker.as_ref().unwrap().query, "01");
        let _ = update(&mut s, Message::Opened(Ok(())));
        assert!(s.usage.boost(Kind::File, &file, &query, usage::now()) > 0);
    }

    #[test]
    fn focused_picker_captures_numbered_keys_before_the_filter_input() {
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        use iced::keyboard::key::{Code, Physical};
        use iced::{Rectangle, Size};

        for (symbol, code, modifiers, slot) in [
            ("1", Code::Digit1, Modifiers::CTRL, Some(0)),
            ("é", Code::Digit2, Modifiers::CTRL, Some(1)),
            ("7", Code::Digit7, Modifiers::empty(), None),
        ] {
            let mut s = picker_state();
            let generation = s.application_generation;
            let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(12)) });
            let _ = command(&mut s, Command::Last);
            let messages = {
                let renderer =
                    iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
                let mut element = view(&s);
                let widget = element.as_widget_mut();
                let mut tree = Tree::new(&*widget);
                let bounds = Size::new(720.0, 500.0);
                let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
                let layout = Layout::new(&node);
                widget.operate(
                    &mut tree,
                    layout,
                    &renderer,
                    &mut iced::advanced::widget::operation::focusable::focus::<()>(open_with::input_id()),
                );
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                for event in [
                    Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: Key::Character(symbol.into()),
                        modified_key: Key::Character(symbol.into()),
                        physical_key: Physical::Code(code),
                        location: keyboard::Location::Standard,
                        modifiers,
                        text: Some(symbol.into()),
                        repeat: false,
                    }),
                ] {
                    widget.update(
                        &mut tree,
                        &event,
                        layout,
                        mouse::Cursor::Unavailable,
                        &renderer,
                        &mut iced::advanced::clipboard::Null,
                        &mut shell,
                        &Rectangle::with_size(bounds),
                    );
                }
                assert_eq!(shell.event_status(), iced::event::Status::Captured);
                messages
            };
            assert_eq!(messages.len(), 1);
            match slot {
                Some(slot) => assert!(matches!(&messages[0], Message::Cmd(Command::ActivateVisible(i)) if *i == slot)),
                None => assert!(
                    matches!(&messages[0], Message::OpenWith { event: open_with::Event::Query(query), .. } if query == symbol)
                ),
            }
            for message in messages {
                let _ = update(&mut s, message);
            }
            assert_eq!(s.application_picker.as_ref().unwrap().query, if slot.is_some() { "" } else { symbol });
            assert_eq!(s.opening.is_some(), slot.is_some());
            assert_eq!(s.query, "report");
        }
    }

    #[test]
    fn picker_numbered_commands_resolve_current_filtered_and_scrolled_app_ids() {
        let file = file_hits().remove(0);
        let mut panel = open_with::Panel::new(file);
        panel.loaded(Ok(picker_choices(12)), &mut Icons::disabled());
        let _ = panel.command(Command::Last);
        assert!(
            matches!(panel.command(Command::ActivateVisible(0)), open_with::Effect::Open(id) if id == "editor-06.desktop")
        );
        assert!(matches!(panel.command(Command::ActivateVisible(6)), open_with::Effect::None));
        let _ = panel.event(open_with::Event::Query("07".into()));
        assert!(
            matches!(panel.command(Command::ActivateVisible(0)), open_with::Effect::Open(id) if id == "editor-07.desktop")
        );
    }

    #[test]
    fn picker_numbered_choice_remains_selected_after_a_failed_open() {
        for query in ["", "editor 0"] {
            let mut panel = open_with::Panel::new(file_hits().remove(0));
            panel.loaded(Ok(picker_choices(12)), &mut Icons::disabled());
            let _ = panel.event(open_with::Event::Query(query.into()));
            let _ = panel.command(Command::Last);
            let open_with::Effect::Open(chosen) = panel.command(Command::ActivateVisible(0)) else {
                panic!("the first visible application must be available");
            };
            panel.opening();
            panel.failed("Application could not be started".into());
            assert!(
                matches!(panel.command(Command::Activate), open_with::Effect::Open(id) if id == chosen),
                "Enter must retry {chosen} after its numbered launch failed, query: {query:?}"
            );
        }
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
    fn focus_loss_closes_unless_an_open_folder_settings_or_a_copy_must_finish() {
        fn exits(task: Task<Message>) -> bool {
            let actions = iced::futures::executor::block_on(async {
                match iced_runtime::task::into_stream(task) {
                    Some(stream) => stream.collect::<Vec<_>>().await,
                    None => Vec::new(),
                }
            });
            actions.iter().any(|action| matches!(action, iced_runtime::Action::Exit))
        }
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        assert!(exits(update(&mut s, Message::FocusLost)));

        let mut s = search_state();
        s.opening = Some(("report.desktop".into(), "report".into()));
        assert!(!exits(update(&mut s, Message::FocusLost)), "a pending open reports its own result");

        let mut s = search_state();
        let _ = update(&mut s, Message::ShowFolders);
        assert!(!exits(update(&mut s, Message::FocusLost)), "unsaved folder edits stay open");
        assert!(s.folder_settings.is_some());

        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let _ = s.perform(Action::CopyName);
        let _ = update(&mut s, Message::Query("other".into()));
        assert!(!exits(update(&mut s, Message::FocusLost)), "the launcher still serves the copied name");
    }

    #[test]
    fn copying_or_cutting_input_text_survives_focus_loss() {
        use iced::advanced::{Layout, Shell, clipboard, layout, widget::Tree};
        use iced::keyboard::key::{Code, Physical};
        use iced::{Rectangle, Size};

        #[derive(Default)]
        struct Clipboard(Option<String>);
        impl clipboard::Clipboard for Clipboard {
            fn read(&self, _kind: clipboard::Kind) -> Option<String> {
                self.0.clone()
            }
            fn write(&mut self, kind: clipboard::Kind, contents: String) {
                assert_eq!(kind, clipboard::Kind::Standard);
                self.0 = Some(contents);
            }
        }

        for (letter, code, modifiers) in [
            ("c", Code::KeyC, Modifiers::CTRL),
            ("x", Code::KeyX, Modifiers::CTRL),
            ("c", Code::KeyC, Modifiers::CTRL | Modifiers::SHIFT),
        ] {
            let mut s = search_state();
            let _ = update(&mut s, Message::Query("report".into()));
            let mut clipboard = Clipboard::default();
            let messages = {
                let renderer =
                    iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
                let mut element = view(&s);
                let widget = element.as_widget_mut();
                let mut tree = Tree::new(&*widget);
                let bounds = Size::new(720.0, 500.0);
                let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
                widget.operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()),
                );
                let pressed = |letter: &str, code, modifiers| {
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: Key::Character(letter.into()),
                        modified_key: Key::Character(letter.into()),
                        physical_key: Physical::Code(code),
                        location: keyboard::Location::Standard,
                        modifiers,
                        text: None,
                        repeat: false,
                    })
                };
                let mut messages = Vec::new();
                for event in [
                    Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL)),
                    pressed("a", Code::KeyA, Modifiers::CTRL),
                    Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
                    pressed(letter, code, modifiers),
                ] {
                    let mut shell = Shell::new(&mut messages);
                    widget.update(
                        &mut tree,
                        &event,
                        Layout::new(&node),
                        mouse::Cursor::Unavailable,
                        &renderer,
                        &mut clipboard,
                        &mut shell,
                        &Rectangle::with_size(bounds),
                    );
                    if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                        messages.push(message);
                    }
                }
                messages
            };
            assert_eq!(clipboard.0.as_deref(), Some("report"), "Ctrl+{letter} must copy the actual input selection");
            for message in messages {
                let _ = update(&mut s, message);
            }
            let task = update(&mut s, Message::FocusLost);
            let actions = iced::futures::executor::block_on(async {
                match iced_runtime::task::into_stream(task) {
                    Some(stream) => stream.collect::<Vec<_>>().await,
                    None => Vec::new(),
                }
            });
            assert!(
                !actions.iter().any(|action| matches!(action, iced_runtime::Action::Exit)),
                "{modifiers:?}+{letter} clipboard contents must remain available after clicking the paste destination"
            );
        }
    }

    #[test]
    fn result_copy_path_preserves_non_utf8_files_and_folders() {
        use std::os::unix::ffi::OsStringExt;

        for (bytes, url, expected) in [
            (b"/synthetic/non\xff.txt".as_slice(), "file:///synthetic/non%FF.txt", "file:///synthetic/non%FF.txt"),
            (
                "/synthetic/caffè notes.txt".as_bytes(),
                "file:///synthetic/caff%C3%A8%20notes.txt",
                "/synthetic/caffè notes.txt",
            ),
        ] {
            for is_dir in [false, true] {
                for from_panel in [false, true] {
                    let mut s = search_state();
                    let _ = update(&mut s, Message::Scope(Scope::Files));
                    let _ = update(&mut s, Message::Query("synthetic".into()));
                    let mut file = file_hits().remove(0);
                    file.id = url.into();
                    file.path = Some(std::ffi::OsString::from_vec(bytes.to_vec()).into());
                    file.is_dir = is_dir;
                    let target = Target::File(file.clone());
                    let generation = s.generation;
                    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![file])) });
                    let task = if from_panel {
                        let _ = command(&mut s, Command::OpenActions);
                        update(&mut s, Message::PanelClicked { target, action: Action::CopyPath })
                    } else {
                        command(&mut s, Command::CopyPath)
                    };
                    let actions = iced::futures::executor::block_on(async {
                        iced_runtime::task::into_stream(task).unwrap().collect::<Vec<_>>().await
                    });
                    let copied: Vec<_> = actions
                        .iter()
                        .filter_map(|action| match action {
                            iced_runtime::Action::Clipboard(iced_runtime::clipboard::Action::Write {
                                contents,
                                ..
                            }) => Some(contents.as_str()),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(copied, [expected], "is_dir={is_dir}, from_panel={from_panel}");
                    assert_eq!(
                        s.status,
                        Status::Notice(format!("Copied {}", if expected == url { "file URL" } else { "path" }))
                    );
                    assert!(s.clipboard_owner);
                    assert!(s.opening.is_none());
                }
            }
        }
    }

    #[test]
    fn captured_input_copy_also_runs_the_copy_path_command() {
        use iced::keyboard::key::{Code, Physical};

        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        let expected = match &s.selected().unwrap().target {
            Target::File(file) => file.path.as_ref().unwrap().to_string_lossy().into_owned(),
            _ => panic!("a file must be selected"),
        };
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Character("c".into()),
            modified_key: Key::Character("C".into()),
            physical_key: Physical::Code(Code::KeyC),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::CTRL | Modifiers::SHIFT,
            text: None,
            repeat: false,
        });
        let message = on_event(event, event::Status::Captured, iced::window::Id::unique()).unwrap();
        let task = update(&mut s, message);
        let actions = iced::futures::executor::block_on(async {
            iced_runtime::task::into_stream(task).unwrap().collect::<Vec<_>>().await
        });
        assert!(actions.iter().any(|action| matches!(
            action,
            iced_runtime::Action::Clipboard(iced_runtime::clipboard::Action::Write { contents, .. })
                if *contents == expected
        )));
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
            .map(|(i, name)| ferroseek_desktop::apps::DesktopAction {
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
    fn application_confirmation_keeps_the_original_target_and_query() {
        let root = tempfile::tempdir().unwrap();
        let mut s = search_state();
        s.usage = Usage::load(Some(root.path().join("usage")));
        s.apps.push(
            crate::apps::parse(
                "other.desktop",
                std::path::Path::new("/synthetic/other.desktop"),
                "[Desktop Entry]\nType=Application\nName=Other\nExec=missing\n",
                &[],
                &[],
            )
            .unwrap(),
        );
        let _ = update(&mut s, Message::Query("report".into()));
        let _pending = command(&mut s, Command::Activate);
        assert_eq!(s.usage.boost(Kind::App, "report.desktop", "report", usage::now()), 0);
        let _ = update(&mut s, Message::Query("other".into()));
        assert_eq!(command(&mut s, Command::Activate).units(), 0, "a second Enter must not start another open");
        let _ = update(&mut s, Message::ApplicationOpened { id: "other.desktop".into(), result: Ok(()) });
        assert_eq!(s.usage.boost(Kind::App, "other.desktop", "other", usage::now()), 0);
        let _ = update(&mut s, Message::ApplicationOpened { id: "report.desktop".into(), result: Ok(()) });
        let now = usage::now();
        assert!(
            s.usage.boost(Kind::App, "report.desktop", "report", now)
                > s.usage.boost(Kind::App, "report.desktop", "unrelated", now)
        );
        assert_eq!(s.usage.boost(Kind::File, "report.desktop", "report", now), 0);
        assert_eq!(s.usage.boost(Kind::App, "other.desktop", "other", now), 0);
        let boost = s.usage.boost(Kind::App, "report.desktop", "report", now);
        let _ = update(&mut s, Message::ApplicationOpened { id: "report.desktop".into(), result: Ok(()) });
        assert_eq!(s.usage.boost(Kind::App, "report.desktop", "report", now), boost);
    }

    #[test]
    fn application_submission_error_keeps_actions_usable_without_recording_usage() {
        let root = tempfile::tempdir().unwrap();
        let mut s = application_actions(&["New window"]);
        s.usage = Usage::load(Some(root.path().join("usage")));
        let id = s.apps[0].id.clone();
        let _pending = s.perform(Action::Desktop(0));
        let _ =
            update(&mut s, Message::ApplicationOpened { id: id.clone(), result: Err("Cannot launch target".into()) });
        assert!(s.panel.is_some() && s.opening.is_none());
        assert!(matches!(s.status, Status::Error(_)));
        assert_eq!(s.usage.boost(Kind::App, &id, "report", usage::now()), 0);
        assert!(s.perform(Action::Desktop(0)).units() > 0, "an explicitly retried action is permitted");
    }

    #[test]
    fn closing_aborts_deferred_file_actions() {
        for route in [Action::Open, Action::Reveal, Action::OpenWith] {
            for close in ["escape", "toggle", "drop", "complete"] {
                let mut s = search_state();
                let _ = update(&mut s, Message::Query("report".into()));
                let generation = s.generation;
                let files = file_hits();
                let target = Target::File(files[0].clone());
                let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files)) });
                let _ = update(&mut s, Message::Selected(target));
                let task = if route == Action::OpenWith {
                    let _ = s.perform(route);
                    let generation = s.application_generation;
                    let _ = update(
                        &mut s,
                        Message::ApplicationsLoaded {
                            generation,
                            result: Ok(vec![crate::backend::ApplicationChoice {
                                id: "chosen.desktop".into(),
                                name: "Chosen".into(),
                                icon: String::new(),
                            }]),
                        },
                    );
                    command(&mut s, Command::Activate)
                } else {
                    s.perform(route)
                };
                if close == "escape" {
                    // Close chooser, clear query, then close the launcher.
                    for _ in 0..3 {
                        let _ = command(&mut s, Command::Back);
                    }
                } else if close == "toggle" {
                    let _ = update(&mut s, Message::Ui(UiEvent::Toggle));
                }
                let mut state = Some(s);
                if close == "drop" {
                    drop(state.take());
                }
                let output = iced::futures::executor::block_on(async {
                    if let Some(stream) = iced_runtime::task::into_stream(task) {
                        stream.collect::<Vec<_>>().await
                    } else {
                        Vec::new()
                    }
                });
                if close == "complete" {
                    assert_eq!(output.len(), 1, "control must observe backend failure");
                    let state = state.as_mut().unwrap();
                    for action in output {
                        if let iced_runtime::Action::Output(message) = action {
                            let _ = update(state, message);
                        }
                    }
                    assert!(state.opening.is_none());
                    assert!(matches!(state.status, Status::Error(_)));
                } else {
                    assert!(output.is_empty(), "closing must cancel {route:?} before submission ({close})");
                }
            }
        }
    }

    #[test]
    fn application_tasks_preserve_the_target_and_respect_closing() {
        let case = std::env::var("FERROSEEK_TEST_CANCEL_APP");
        let Ok(case) = case else {
            let root = tempfile::tempdir().unwrap();
            for case in ["escape", "toggle", "dispatch-app", "dispatch-action"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "ui::tests::application_tasks_preserve_the_target_and_respect_closing",
                        "--nocapture",
                    ])
                    .env("FERROSEEK_TEST_CANCEL_APP", case)
                    .env("FERROSEEK_TEST_CANCEL_ROOT", root.path())
                    .env("FERROSEEK_TEST_DISPATCH_OUTPUT", root.path().join(case))
                    .env("HOME", root.path())
                    .env("PATH", "/usr/bin:/bin")
                    .env("XDG_CONFIG_HOME", root.path().join("config"))
                    .env("XDG_DATA_HOME", root.path().join("data"))
                    .env("XDG_STATE_HOME", root.path().join("state"))
                    .env("XDG_CACHE_HOME", root.path().join("cache"))
                    .env("XDG_RUNTIME_DIR", root.path().join("runtime"))
                    .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-ferroseek-test-bus")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{case}: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        };
        let root = std::path::PathBuf::from(std::env::var_os("FERROSEEK_TEST_CANCEL_ROOT").unwrap());
        let dispatch = case.starts_with("dispatch-");
        let mut s = if dispatch { application_actions(&["New window"]) } else { search_state() };
        if dispatch {
            use std::os::unix::fs::PermissionsExt;
            let capture = root.join(format!("capture-{case}"));
            std::fs::write(&capture, "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > \"$FERROSEEK_TEST_DISPATCH_OUTPUT.tmp\"\nmv \"$FERROSEEK_TEST_DISPATCH_OUTPUT.tmp\" \"$FERROSEEK_TEST_DISPATCH_OUTPUT\"\n").unwrap();
            std::fs::set_permissions(&capture, std::fs::Permissions::from_mode(0o700)).unwrap();
            s.apps[0].exec = format!("\"{}\" captured-app", capture.display());
            s.apps[0].actions[0].exec = format!("\"{}\" captured-action", capture.display());
            s.apps.push(
                crate::apps::parse(
                    "other.desktop",
                    std::path::Path::new("/synthetic/other.desktop"),
                    &format!(
                        "[Desktop Entry]\nType=Application\nName=Other\nExec=\"{}\" wrong-target\n",
                        capture.display()
                    ),
                    &[],
                    &[],
                )
                .unwrap(),
            );
            s.usage = Usage::load(Some(root.join(format!("usage-{case}"))));
        } else {
            s.apps[0].exec = format!("\"{}\"", root.join("nonexistent-command").display());
        }
        let _ = update(&mut s, Message::Query("report".into()));
        let task =
            if case == "dispatch-action" { s.perform(Action::Desktop(0)) } else { command(&mut s, Command::Activate) };
        if dispatch {
            let _ = update(&mut s, Message::Query("other".into()));
            assert_eq!(command(&mut s, Command::Activate).units(), 0);
        } else if case == "escape" {
            let _ = update(&mut s, Message::Query(String::new()));
            let _ = command(&mut s, Command::Back);
        } else {
            let _ = update(&mut s, Message::Ui(UiEvent::Toggle));
        }
        let output = iced::futures::executor::block_on(async {
            if let Some(stream) = iced_runtime::task::into_stream(task) {
                stream.collect::<Vec<_>>().await
            } else {
                Vec::new()
            }
        });
        if dispatch {
            assert_eq!(output.len(), 1);
            for action in output {
                if let iced_runtime::Action::Output(message) = action {
                    let _ = update(&mut s, message);
                }
            }
            let path = std::path::PathBuf::from(std::env::var_os("FERROSEEK_TEST_DISPATCH_OUTPUT").unwrap());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !path.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let expected = if case == "dispatch-action" { "captured-action\n" } else { "captured-app\n" };
            assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
            assert!(s.usage.boost(Kind::App, "report.desktop", "report", usage::now()) > 0);
            assert_eq!(s.usage.boost(Kind::App, "other.desktop", "other", usage::now()), 0);
        } else {
            assert!(output.is_empty(), "closing must abort deferred submission, not produce a launch result");
        }
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
