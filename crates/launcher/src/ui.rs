// SPDX-License-Identifier: GPL-3.0-or-later
//! The launcher window: an iced program on a wlr-layer-shell overlay.

mod alias_editor;
pub(crate) mod clipboard_view;
mod favorites;
mod global_shortcut_settings;
mod saved_editor;
mod saved_results;
mod script_editor;
mod script_prompt;
mod scripts;
mod settings;
mod shortcut_settings;
mod views;
mod windows;
mod workflow_editor;

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PANEL_ID: AtomicU64 = AtomicU64::new(1);

use iced::advanced::layout::{self as widget_layout, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::alignment::{Horizontal, Vertical};
use iced::event::{self, Event};
use iced::futures::channel::mpsc::UnboundedReceiver;
use iced::futures::stream::{self, BoxStream, StreamExt};
use iced::keyboard::{self, Key, Modifiers};
use iced::mouse::{self, ScrollDelta};
use iced::widget::{
    self, Id, button, column, container, image, mouse_area, row, scrollable, space, stack, svg, text, text_input,
};
use iced::{
    Background, Border, Color, Element, Font, Length, Rectangle, Size, Subscription, Task, Theme, Vector, padding,
};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};
use iced_layershell::to_layer_message;

use crate::Destination;
use crate::activation::Activation;
use crate::appearance;
use crate::apps::App;
use crate::backend::{ACTION_OPEN_PARENT, Backend, FileHit, RunError, SearchStatus, UiEvent};
use crate::calc;
use crate::details;
use crate::favorites::Favorites;
use crate::folders;
use crate::icons::{Icon, Icons};
use crate::layout::{self, FOOTER_HEIGHT, INPUT_HEIGHT, PAD, ROW_HEIGHT};
use crate::material::Material;
use crate::model::{self, Action, ClickTracker, Command, Cursor, Item, Scope, Target};
use crate::open_with;
use crate::output;
use crate::preview;
use crate::saved;
use crate::timing;
use crate::usage::{self, Kind, Usage};
use ferroseek_config::appearance::Preferences;
use ferroseek_config::shortcuts::{Action as Shortcut, Chord};
pub(crate) use views::back_refresh_header;

#[cfg(test)]
pub const WIDTH: u32 = 720;
const NARROW_SEARCH_WIDTH: u32 = 336;
#[cfg(test)]
const PREVIEW_WIDTH: u32 = 980;
const VISIBLE_ROWS: usize = model::NUMBERED_SLOTS;
const ICON_SIZE: f32 = 28.0;
const KEYCAP_SIZE: f32 = 11.0;
const KEYCAP_INSET: f32 = 6.0;
const ACTION_ROW_HEIGHT: f32 = 34.0;
const ACTION_ROW_GAP: f32 = 2.0;
/// Below this, file search is not run (the daemon needs two characters).
const MIN_FILE_QUERY: usize = 2;
const UNCONFIRMED_OPEN: &str = "Open unconfirmed. Check the desktop, then reopen FerroSeek.";

#[cfg(test)]
use crate::layout::compact_height;

#[cfg(test)]
fn full_height() -> u32 {
    compact_height() + (ROW_HEIGHT * VISIBLE_ROWS as f32 + PAD) as u32
}

pub(crate) use crate::appearance::palette;

static UI_EVENTS: Mutex<Option<UnboundedReceiver<UiEvent>>> = Mutex::new(None);
static OUTPUT_EVENTS: Mutex<Option<UnboundedReceiver<output::Event>>> = Mutex::new(None);
static BOOT: Mutex<Option<(Launcher, Task<Message>)>> = Mutex::new(None);
static MATERIAL_EVENTS: Mutex<Option<UnboundedReceiver<bool>>> = Mutex::new(None);

/// What the launcher starts with.
pub struct Boot {
    pub config: ferroseek_config::Config,
    pub opaque: bool,
    pub private: bool,
    pub activation: Activation,
    pub backend: Backend,
    pub apps: Vec<App>,
    pub usage: Usage,
    pub favorites: Favorites,
    pub settings_path: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Idle,
    Searching,
    Error(String),
    CatalogError,
    Notice(String),
}

#[derive(Debug, Clone, PartialEq)]
enum SearchStatusPanel {
    Loading,
    Ready(SearchStatus),
    Error(String),
}

struct Panel {
    id: u64,
    title: String,
    context: String,
    actions: Vec<Action>,
    desktop_labels: Vec<String>,
    query: String,
    filter_revision: u64,
    cursor: Cursor,
}

impl Panel {
    fn new(item: &Item, favorite: bool) -> Self {
        Self {
            id: NEXT_PANEL_ID.fetch_add(1, Ordering::Relaxed),
            title: item.title.clone(),
            context: item.subtitle.clone(),
            actions: model::actions_for(&item.target, favorite),
            desktop_labels: Vec::new(),
            query: String::new(),
            filter_revision: 0,
            cursor: Cursor::new(VISIBLE_ROWS),
        }
    }

    fn visible_rows(&self, window_height: u32, result_rows: usize) -> usize {
        // Reserve space for the title, filter and footer controls before
        // fitting the list within the launcher surface.
        let chrome = if self.context.is_empty() { 212 } else { 230 };
        let row = (ACTION_ROW_HEIGHT + ACTION_ROW_GAP) as u32;
        let rows = (window_height.saturating_sub(chrome) / row).clamp(1, model::NUMBERED_SLOTS as u32);
        (rows as usize).min(result_rows)
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
        if self.query != query {
            self.filter_revision += 1;
        }
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
    file_search_succeeded: bool,
    cursor: Cursor,
    selected: Option<Target>,
}

pub struct Launcher {
    output: output::Observer,
    output_bounds: Option<(u32, u32)>,
    placement: Option<layout::Geometry>,
    surface_ready: bool,
    window_height: u32,
    preferences: Preferences,
    system_dark: Option<bool>,
    opaque: bool,
    settings: Option<settings::Entry>,
    shortcuts: shortcut_settings::State,
    global_shortcuts: global_shortcut_settings::State,
    clipboard_history: clipboard_view::State,
    window_switcher: windows::State,
    appearance: Option<appearance::Panel>,
    appearance_generation: u64,
    input_epoch: u64,
    material: Material,
    glass: bool,
    activation: Activation,
    backend: Backend,
    apps: Vec<App>,
    usage: Usage,
    favorites: Favorites,
    saved: saved::Store,
    scripts: scripts::State,
    alias_editor: Option<alias_editor::Editor>,
    saved_editor: Option<saved_editor::Editor>,
    edit_generation: u64,
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
    file_search_succeeded: bool,
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
    deferred_destination: Option<Destination>,
    status_generation: u64,
    generation: u64,
    status: Status,
    compact: bool,
}

#[to_layer_message]
#[derive(Debug, Clone)]
pub enum Message {
    InputApplied,
    InputFocused { epoch: u64, field: Id, at: std::time::Instant },
    MaterialChanged(bool),
    SurfaceResized((u32, u32)),
    Output(output::Event),
    ApplicationOpened { id: String, result: Result<(), String> },
    LinkOpened(Result<(), RunError>),
    Script { generation: u64, event: scripts::Event },
    ScriptEdit { generation: u64, event: script_editor::Event },
    WorkflowEdit { generation: u64, event: workflow_editor::Event },
    WorkflowRemoveStep,
    SavedEditor { generation: u64, event: saved_editor::Event },
    AliasEditor { generation: u64, event: alias_editor::Event },
    TimingFrame(std::time::Instant),
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
    ShowSettings,
    SettingsSelected(settings::Entry),
    ShowSearchStatus,
    ShowFolders,
    ShowAppearance,
    ShortcutSettings { generation: u64, event: shortcut_settings::Event },
    GlobalShortcuts { generation: u64, event: global_shortcut_settings::Event },
    ShortcutRecorded(Result<Chord, &'static str>),
    Clipboard { generation: u64, event: clipboard_view::Event },
    ClipboardShortcut(clipboard_view::Shortcut),
    Windows { generation: u64, event: windows::Event },
    Appearance { generation: u64, event: appearance::Event },
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
    ResultAction { target: Target, action: Action, query: String },
    ToggleActionsButton { target: Target, open: bool, panel_id: Option<u64> },
    EmptyBack { step: EmptyBack, generation: u64 },
    ContextMenu(Target),
    PanelBack { target: Target, filtering: bool, panel_id: u64, filter_revision: u64 },
    PanelRun { target: Target, action: Action, panel_id: u64, filter_revision: u64 },
    PanelClicked { target: Target, action: Action },
    Opened(Result<(), RunError>),
    InputClipboard(Option<Box<Message>>),
    FocusGained,
    FocusLost,
    Ui(UiEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyBack {
    HidePreview,
    ClearSearch,
    PreviousSearch,
    Close,
}

impl EmptyBack {
    fn label(self) -> &'static str {
        match self {
            Self::HidePreview => "Hide preview",
            Self::ClearSearch => "Clear search",
            Self::PreviousSearch => "Previous search",
            Self::Close => "Close",
        }
    }
}

fn empty_back(s: &Launcher) -> EmptyBack {
    if s.preview_visible() {
        EmptyBack::HidePreview
    } else if !s.query.is_empty() {
        EmptyBack::ClearSearch
    } else if s.search_folder.is_some() {
        EmptyBack::PreviousSearch
    } else {
        EmptyBack::Close
    }
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

pub fn run(
    boot_data: Boot,
    destination: Destination,
    events: UnboundedReceiver<UiEvent>,
) -> Result<(), iced_layershell::Error> {
    let activation = boot_data.activation.clone();
    let provisional = layout::calculate(boot_data.config.appearance, None, Some(0), false);
    let (output, output_events) = output::Observer::new((provisional.width, provisional.height));
    let shared_output = output.clone();
    *OUTPUT_EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(output_events);
    let (material, material_events) = Material::new(
        !boot_data.opaque && boot_data.config.appearance.material == ferroseek_config::appearance::Material::Glass,
        (provisional.width, provisional.height),
    );
    let shared_material = material.clone();
    *MATERIAL_EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(material_events);
    *UI_EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(events);
    let (mut state, startup) =
        boot_from(boot_data, material, output, Icons::new(ICON_SIZE as u16)).start_destination(destination);
    let initial = state.placement.expect("startup computes the first surface geometry");
    shared_material.resize((initial.width, initial.height));
    state.surface_ready = true;
    *BOOT.lock().unwrap_or_else(|e| e.into_inner()) = Some((state, startup));
    iced_layershell::application(boot, namespace, update, view)
        .style(|_, theme| iced::theme::Style {
            background_color: Color::TRANSPARENT,
            text_color: palette::TEXT.resolve(theme),
        })
        .theme(|state: &Launcher| state.appearance_theme(state.glass))
        .subscription(subscription)
        .settings(Settings {
            with_connection: Some(iced_layershell::reexport::WithConnection::from(move || {
                let connection = wayland_client::Connection::connect_to_env()?;
                activation.attach(connection.clone(), shared_material.clone(), shared_output.clone());
                Ok(connection)
            })),
            layer_settings: LayerShellSettings {
                anchor: if initial.centered { Anchor::empty() } else { Anchor::Top },
                layer: Layer::Overlay,
                exclusive_zone: 0,
                size: Some((initial.width, initial.height)),
                margin: (initial.top as i32, 0, 0, 0),
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
    BOOT.lock().unwrap_or_else(|e| e.into_inner()).take().expect("boot data set once by run()")
}

fn boot_from(data: Boot, material: Material, output: output::Observer, icons: Icons) -> Launcher {
    Launcher {
        output,
        output_bounds: None,
        placement: None,
        surface_ready: false,
        window_height: layout::compact_height(),
        preferences: data.config.appearance,
        system_dark: None,
        opaque: data.opaque,
        settings: None,
        shortcuts: shortcut_settings::State::new(data.config.shortcuts),
        global_shortcuts: global_shortcut_settings::State::default(),
        clipboard_history: clipboard_view::State::new(data.private),
        window_switcher: windows::State::default(),
        appearance: None,
        appearance_generation: 0,
        input_epoch: 0,
        material,
        glass: false,
        activation: data.activation,
        backend: data.backend,
        apps: data.apps,
        usage: data.usage,
        favorites: data.favorites,
        saved: saved::Store::load(&saved::Store::path(&data.settings_path)),
        scripts: scripts::State::load(&data.settings_path),
        alias_editor: None,
        saved_editor: None,
        edit_generation: 0,
        opening: None,
        launch_task: None,
        unconfirmed_open: false,
        clipboard_owner: false,
        icons,
        query: String::new(),
        scope: Scope::All,
        search_folder: None,
        folder_more: false,
        folder_search_history: Vec::new(),
        scope_generation: 0,
        app_hits: Vec::new(),
        files: Vec::new(),
        file_search_succeeded: false,
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
        window_width: data.config.appearance.width.pixels(),
        text_preview: None,
        preview_generation: 0,
        preview_request: None,
        folder_settings: None,
        folder_generation: 0,
        settings_path: data.settings_path,
        deferred_destination: None,
        status_generation: 0,
        generation: 0,
        status: Status::Idle,
        compact: true,
    }
}

fn subscription(_: &Launcher) -> Subscription<Message> {
    let events = if timing::enabled() { event::listen_raw(on_event) } else { event::listen_with(on_event) };
    Subscription::batch([
        events,
        Subscription::run(ui_events),
        Subscription::run(material_events),
        Subscription::run(output_events),
    ])
}

fn on_event(event: Event, _status: event::Status, _window: iced::window::Id) -> Option<Message> {
    match event {
        Event::Window(iced::window::Event::Resized(size) | iced::window::Event::Opened { size, .. }) => {
            Some(Message::SurfaceResized((size.width.round() as u32, size.height.round() as u32)))
        }
        Event::Window(iced::window::Event::RedrawRequested(at)) if timing::needs_frame() => {
            Some(Message::TimingFrame(at))
        }
        Event::Window(iced::window::Event::Focused) => Some(Message::FocusGained),
        Event::Window(iced::window::Event::Unfocused) => Some(Message::FocusLost),
        _ => None,
    }
}

pub(super) fn input_event(event: &Event, status: event::Status) -> Option<Message> {
    if let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event {
        if status == event::Status::Captured {
            return None;
        }
        let dy = match delta {
            ScrollDelta::Lines { y, .. } => *y,
            ScrollDelta::Pixels { y, .. } => *y / ROW_HEIGHT,
        };
        return (dy != 0.0).then_some(Message::Wheel(dy));
    }
    let Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. }) = event else { return None };
    let command = key_message(key, *modifiers);
    if status == event::Status::Captured
        && modifiers.command()
        && matches!(key.to_latin(*physical_key), Some('c' | 'x'))
    {
        Some(Message::InputClipboard(command.map(Box::new)))
    } else {
        command
    }
}

fn key_message(key: &Key, mods: Modifiers) -> Option<Message> {
    if mods == (Modifiers::CTRL | Modifiers::SHIFT) && matches!(key.as_ref(), Key::Character("d" | "D")) {
        return Some(Message::WorkflowRemoveStep);
    }
    if mods == Modifiers::CTRL {
        match key.as_ref() {
            Key::Character("s" | "S") => return Some(Message::FolderShortcut(folders::Shortcut::Save)),
            Key::Character("d" | "D") => return Some(Message::FolderShortcut(folders::Shortcut::Remove)),
            Key::Character("r" | "R") => return Some(Message::RefreshSearchStatus),
            _ => {}
        }
    }
    model::command_for(key, mods).map(Message::Cmd)
}

fn output_events() -> BoxStream<'static, Message> {
    match OUTPUT_EVENTS.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(rx) => rx.map(Message::Output).boxed(),
        None => stream::pending().boxed(),
    }
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
    fn start_destination(mut self, destination: Destination) -> (Self, Task<Message>) {
        self.rebuild(false);
        let prefill = if !self.settings_path.exists() {
            self.deferred_destination = Some(destination);
            self.show_folders(true)
        } else if matches!(&destination, Destination::Search(query) if query.is_empty()) {
            Task::none()
        } else {
            self.open_startup_destination(destination)
        };
        let startup = Task::batch([self.resize(), prefill]);
        (self, startup)
    }

    fn resume_query(&mut self, query: String) -> Task<Message> {
        self.panel = None;
        let change = update(self, Message::Query(query));
        Task::batch([change, crate::input::focus(search_id()), widget::operation::move_cursor_to_end(search_id())])
    }

    fn appearance_theme(&self, glass: bool) -> Theme {
        appearance::theme(self.preferences, self.system_dark, glass)
    }
    fn appearance_event(&mut self, event: appearance::Event) -> Task<Message> {
        let ready = matches!(&event, appearance::Event::Loaded(Ok(_)));
        let saved = matches!(&event, appearance::Event::Saved(Ok(_)));
        let (task, close) = self.appearance.as_mut().unwrap().update(event);
        if ready {
            timing::event("appearance_ready");
        }
        if saved {
            timing::event("appearance_saved");
        }
        self.finish_appearance(task, close)
    }
    fn finish_appearance(&mut self, task: Task<appearance::Event>, close: Option<Preferences>) -> Task<Message> {
        let generation = self.appearance_generation;
        timing::appearance_changed(close.is_none());
        self.preferences = close.unwrap_or_else(|| self.appearance.as_ref().unwrap().draft);
        timing::event(if self.preferences.dark(self.system_dark) { "appearance_dark" } else { "appearance_light" });
        self.material
            .set_allowed(!self.opaque && self.preferences.material == ferroseek_config::appearance::Material::Glass);
        let task = task.map(move |event| Message::Appearance { generation, event });
        if close.is_some() {
            self.appearance = None;
            timing::event("appearance_closed");
            self.appearance_generation += 1;
            Task::batch([task, self.return_to_parent()])
        } else {
            Task::batch([task, self.resize()])
        }
    }

    fn home_results(&self) -> Vec<Item> {
        let catalog = model::Catalog { apps: &self.apps, saved: &self.saved, scripts: &self.scripts.catalog };
        let limit = if self.scope == Scope::Commands { usize::MAX } else { VISIBLE_ROWS };
        model::home_items(
            catalog,
            &self.usage.recent(usage::now(), usize::MAX),
            self.favorites.items(),
            self.scope,
            limit,
        )
    }

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
            self.home_results()
        } else {
            let mut items: Vec<Item> = if self.scope.shows_calculator(&self.query) {
                model::calc_item(&self.query).into_iter().collect()
            } else {
                Vec::new()
            };
            if !calc::is_explicit(&self.query) {
                items.extend(model::command_items(self.scope, &self.query));
                let mut files = self.files.clone();
                let now = usage::now();
                model::order_files(&mut files, |f| self.usage.boost(Kind::File, &f.id, &self.query, now));
                items.extend(model::build_items(&self.apps, &self.app_hits, &files));
            }
            items
        };
        self.merge_saved_entries();
        self.merge_alias();
        self.append_search_fallbacks();
        self.sync_result_icons();
        let kept = previous.and_then(|t| self.items.iter().position(|i| i.target.same_identity(&t)));
        match kept {
            Some(j) => self.cursor.select(j, self.items.len()),
            None => {
                self.cursor.reset();
                self.panel = None;
            }
        }
    }

    fn sync_result_icons(&mut self) {
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
                    Target::Command(_) => &["preferences-system", "applications-system"],
                    Target::Script(_) => &["utilities-terminal", "application-x-executable"],
                    Target::Workflow(_) => &["system-run", "application-x-executable"],
                    Target::Link(..) => &["insert-link"],
                    Target::Snippet(_) => &["text-x-generic"],
                    Target::UnavailableAlias(_) => &["dialog-information"],
                };
                icons.get(&item.icon, fallbacks)
            })
            .collect();
    }

    fn home_rows(&self) -> Option<usize> {
        // Keep a stable viewport while typing or showing a secondary view.
        let home = self.search_folder.is_none()
            && self.query.trim().is_empty()
            && !self.search_suspended()
            && !self.preview_enabled
            && self.panel.is_none();
        let needs_room = matches!(self.status, Status::Searching)
            || (self.items.is_empty()
                && matches!(self.status, Status::Error(_) | Status::CatalogError | Status::Notice(_)));
        (home && !needs_room).then_some(self.items.len())
    }

    fn resize(&mut self) -> Task<Message> {
        let mut geometry =
            layout::calculate(self.preferences, self.output_bounds, self.home_rows(), self.preview_visible());
        if let Some(height) = self.dialog_height() {
            geometry = geometry.with_content_height(height, self.output_bounds);
        }
        self.resize_clipboard(geometry.height);
        self.window_switcher.resize(geometry.height);
        self.cursor.visible = geometry.rows.max(1);
        self.cursor.move_by(0, self.items.len());
        if let Some(panel) = &mut self.panel {
            panel.cursor.visible = panel.visible_rows(geometry.height, geometry.rows);
            let actions = panel.filtered_actions().len();
            panel.cursor.move_by(0, actions);
        }
        if let Some(panel) = &mut self.application_picker {
            panel.resize(geometry.height, self.opening.is_some());
        }
        if self.placement == Some(geometry) {
            return Task::none();
        }
        self.compact = geometry.rows == 0;
        self.window_width = geometry.width;
        self.window_height = geometry.height;
        self.placement = Some(geometry);
        self.output.resize((geometry.width, geometry.height));
        timing::layout(geometry);
        if !self.surface_ready {
            // Startup passes the computed size directly to the layer surface.
            return Task::none();
        }
        let anchor = if geometry.centered { Anchor::empty() } else { Anchor::Top };
        let size = Task::done(Message::AnchorSizeChange(anchor, (geometry.width, geometry.height)));
        let margin = Task::done(Message::MarginChange((geometry.top as i32, 0, 0, 0)));
        if geometry.centered { size.chain(margin) } else { margin.chain(size) }
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
            (Action::EditAlias, _) => self.open_alias_editor(&item),
            (Action::GlobalShortcut, _) => self.show_command_shortcut(&item),
            (Action::OpenCommand | Action::RunScript | Action::EditScript | Action::EditWorkflow, _) => {
                self.activate_command(&item.target, action)
            }
            (Action::OpenLink, Target::Link(link, terms)) => self.open_saved_link(link, terms),
            (Action::EditLink, Target::Link(link, _)) => {
                self.open_saved_editor(saved_editor::Kind::Link, Some(saved_editor::Entry::Link(link.clone())))
            }
            (Action::AddFavorite | Action::RemoveFavorite | Action::MoveFavoriteUp | Action::MoveFavoriteDown, _) => {
                self.favorite_action(&item.target, action)
            }
            (_, Target::Snippet(snippet)) => self.snippet_action(action, snippet),
            (Action::Launch | Action::Desktop(_), Target::App(i)) => self.open_application(*i, action),
            (Action::Open | Action::Reveal, Target::File(file)) => self.open_indexed_file(file, action),
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
                Task::batch([load, self.resize(), crate::input::focus(open_with::input_id())])
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
                self.resume_query(query)
            }
            (Action::ResetRanking, _) => self.reset_ranking(&item.target),
            _ => Task::none(),
        }
    }

    fn open_indexed_file(&mut self, file: &FileHit, action: Action) -> Task<Message> {
        let operation = if action == Action::Reveal { ACTION_OPEN_PARENT } else { "" };
        self.opening = Some((file.id.clone(), self.query.clone()));
        self.status = Status::Notice("Opening…".into());
        let (task, handle) =
            Task::perform(self.backend.clone().run(file.id.clone(), operation), Message::Opened).abortable();
        self.launch_task = Some(handle.abort_on_drop());
        task
    }

    fn open_application(&mut self, index: usize, action: Action) -> Task<Message> {
        let app = self.apps[index].clone();
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
                let app_id = if app.terminal { String::new() } else { app.id.trim_end_matches(".desktop").to_string() };
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

    fn actions_for(&self, target: &Target) -> Vec<Action> {
        model::actions_for(target, self.is_favorite(target))
    }

    fn panel_for(&self, item: &Item) -> Panel {
        let favorite = self.is_favorite(&item.target);
        let mut panel = Panel::new(item, favorite);
        panel.cursor.visible = panel.visible_rows(self.window_height, self.cursor.visible);
        if let Target::App(i) = item.target
            && let Some(app) = self.apps.get(i)
        {
            panel.actions.extend((0..app.actions.len()).map(Action::Desktop));
            panel.desktop_labels = app.actions.iter().map(|action| action.name.clone()).collect();
        }
        if matches!(item.target, Target::App(_)) {
            panel.actions.push(Action::EditAlias);
        }
        let runnable_local = self.scripts.history_key(&item.target).is_some();
        if runnable_local {
            panel.actions.push(Action::ResetRanking);
            panel.actions.push(if favorite { Action::RemoveFavorite } else { Action::AddFavorite });
        }
        self.add_favorite_move_actions(&item.target, &mut panel.actions);
        panel.actions.extend([Action::EditAlias, Action::GlobalShortcut].into_iter().filter(|_| runnable_local));
        panel
    }

    fn reset_ranking(&mut self, target: &Target) -> Task<Message> {
        let history = match target {
            Target::App(i) => Some((Kind::App, self.apps[*i].id.as_str())),
            Target::File(f) => Some((Kind::File, f.id.as_str())),
            Target::Command(command) => Some((Kind::Command, command.id())),
            _ => self.scripts.history_key(target).map(|key| (Kind::Command, key)),
        };
        let Some((kind, key)) = history else { return Task::none() };
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
        Task::batch([self.resize(), crate::input::focus(search_id())])
    }

    /// Copies and stays open: on Wayland the clipboard is served by its
    /// owner, so exiting right away could lose it without a clipboard
    /// manager.
    fn copy(&mut self, s: String, what: &str) -> Task<Message> {
        self.status = Status::Notice(format!("Copied {what}"));
        self.panel = None;
        Task::batch([self.write_clipboard(s), crate::input::focus(search_id())])
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
            file_search_succeeded: self.file_search_succeeded,
            cursor: self.cursor,
            selected: self.selected().map(|item| item.target.clone()),
        });
        self.search_folder = Some(folder);
        self.scope_generation += 1;
        self.scope = Scope::Files;
        let query = update(self, Message::Query(String::new()));
        Task::batch([query, crate::input::focus(search_id())])
    }

    fn leave_folder_search(&mut self) {
        if self.search_folder.take().is_some() {
            self.folder_search_history.clear();
            self.scope_generation += 1;
        }
    }

    fn restore_folder_search(&mut self, frame: FolderSearchFrame) {
        self.query = frame.query;
        self.scope = frame.scope;
        self.search_folder = frame.folder;
        self.folder_more = frame.more;
        self.files = frame.files;
        self.file_search_succeeded = frame.file_search_succeeded;
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
            crate::input::focus(search_id()),
            widget::operation::move_cursor_to_end(search_id()),
        ])
    }

    fn search_suspended(&self) -> bool {
        self.file_details.is_some()
            || self.application_picker.is_some()
            || self.search_status.is_some()
            || self.settings.is_some()
            || self.settings_form_open()
            || self.saved_editor_open()
            || self.command_view_open()
    }

    fn settings_form_open(&self) -> bool {
        self.folder_settings.is_some()
            || self.appearance.is_some()
            || self.shortcuts.panel.is_some()
            || self.global_shortcuts.panel.is_some()
    }

    fn saved_editor_open(&self) -> bool {
        self.saved_editor.is_some() || self.alias_editor.is_some()
    }

    fn preview_visible(&self) -> bool {
        self.preview_enabled && !self.search_suspended()
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
        Task::batch([load, self.resize(), crate::input::focus(search_id())])
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
                Task::batch([self.resize(), crate::input::focus(search_id())])
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
                Task::batch([self.resize(), crate::input::focus(search_id())])
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
            open_with::Effect::Focus => crate::input::focus(open_with::input_id()),
            open_with::Effect::Close => {
                if let Some(panel) = self.application_picker.take() {
                    self.cursor.select_target(&Target::File(panel.file), &self.items);
                }
                self.application_generation += 1;
                Task::batch([self.resize(), crate::input::focus(search_id())])
            }
            open_with::Effect::Reload => {
                if self.opening.is_some() || !self.application_picker.as_mut().is_some_and(|panel| panel.loading()) {
                    return Task::none();
                }
                Task::batch([self.request_applications(), self.resize(), crate::input::focus(open_with::input_id())])
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
                Task::batch([task, self.resize()])
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
        self.settings = self.settings.map(|_| settings::Entry::SearchFolders);
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
                let refresh = if refresh { query_changed(self, self.query.clone()) } else { Task::none() };
                Task::batch([task, refresh, self.return_to_parent()])
            }
            folders::Effect::Applied => {
                self.settings = None;
                self.folder_settings = None;
                self.folder_generation += 1;
                let root = self.folder_search_history.drain(..).next();
                if let Some(root) = root {
                    self.restore_folder_search(root);
                }
                let destination =
                    self.deferred_destination.take().unwrap_or_else(|| Destination::Search(self.query.clone()));
                let open = self.open_setup_destination(destination);
                let focus = self.input_focus().map_or_else(Task::none, crate::input::focus);
                Task::batch([task, open, self.resize(), focus])
            }
        }
    }
}

fn update(s: &mut Launcher, message: Message) -> Task<Message> {
    let (previous_preview, previous_home) = (s.preview_context(), s.home_rows());
    let query_boundary = matches!(&message, Message::Query(_));
    let task = match message {
        Message::Ui(UiEvent::OpenCommand(request)) => s.deliver_command(request),
        Message::FolderListed { generation, result } if generation == s.generation => {
            let result = result.map(|(files, more)| {
                s.folder_more = more;
                Some(files)
            });
            files_loaded(s, generation, result)
        }
        Message::FolderListed { .. } => Task::none(),
        message => update_inner(s, message),
    };
    if previous_preview.1 && !s.preview_visible() && s.preview_enabled {
        let loading = s.preview_request.is_some();
        s.cancel_preview(loading);
    }
    if (previous_preview, previous_home) != (s.preview_context(), s.home_rows()) || query_boundary {
        let preview = s.sync_preview();
        Task::batch([task, preview, s.resize()])
    } else {
        task
    }
}

fn update_inner(s: &mut Launcher, message: Message) -> Task<Message> {
    if let Message::InputApplied = message {
        s.input_epoch = s.input_epoch.wrapping_add(1);
        return Task::none();
    }
    if let Message::InputFocused { epoch, field, at } = message {
        if epoch == s.input_epoch {
            if field == actions_id() && s.panel.is_some() {
                timing::event_at("actions_focused", at);
            } else if field == search_id() {
                timing::event_at("search_focused", at);
            }
        }
        return Task::none();
    }
    if let Message::Output(event) = message {
        return match event {
            output::Event::Mapped => Task::done(Message::SizeChange((s.window_width, s.window_height))),
            output::Event::Bounds(bounds) => {
                s.output_bounds = bounds;
                timing::output(bounds);
                s.resize()
            }
        };
    }
    if let Message::SurfaceResized(size) = message {
        s.material.resize(size);
        s.output.resize(size);
        timing::surface_resized(size);
        return Task::none();
    }
    if let Message::MaterialChanged(enabled) = message {
        s.glass = enabled;
        timing::event(if enabled { "material_blur_requested" } else { "material_opaque" });
        return Task::none();
    }
    if let Message::TimingFrame(at) = message {
        timing::appearance_frame(at);
        let visible = s.panel.is_none() && !s.search_suspended();
        timing::frame(at, s.generation, visible);
        return Task::none();
    }
    update_active_view(s, message)
}

fn update_active_view(s: &mut Launcher, message: Message) -> Task<Message> {
    if s.scripts.is_open() {
        s.update_scripts(message)
    } else if s.window_switcher.panel.is_some() {
        s.update_windows(message)
    } else if s.clipboard_history.panel.is_some() {
        s.update_clipboard_history(message)
    } else if s.alias_editor.is_some() {
        s.update_alias_editor(message)
    } else if s.saved_editor.is_some() {
        s.update_saved_editor(message)
    } else if s.settings_form_open() {
        update_settings_form(s, message)
    } else {
        update_content(s, message)
    }
}

fn update_settings_form(s: &mut Launcher, message: Message) -> Task<Message> {
    if s.appearance.is_some() {
        update_appearance(s, message)
    } else if s.shortcuts.panel.is_some() {
        s.update_shortcut_settings(message)
    } else if s.global_shortcuts.panel.is_some() {
        s.update_global_shortcuts(message)
    } else {
        update_content(s, message)
    }
}

fn update_content(s: &mut Launcher, message: Message) -> Task<Message> {
    if blocks_search_input(s, &message) {
        return Task::none();
    }
    match message {
        Message::PreviewLoaded { generation, result } => s.preview_loaded(generation, result),
        Message::Preview { generation, event } => s.preview_event(generation, event),
        Message::RefreshSearchStatus => refresh_current_view(s),
        Message::Wheel(_) if s.preview_visible() && s.panel.is_none() => Task::none(),
        Message::ResultsWheel(_) if !s.preview_visible() => Task::none(),
        Message::ResultsWheel(dy) => move_results(s, dy),
        Message::FolderBack { generation } => {
            if generation == s.scope_generation {
                s.back_folder_search()
            } else {
                Task::none()
            }
        }
        Message::DetailsLoaded { generation, result } => s.details_loaded(generation, result),
        Message::Details { generation, event } => s.details_event(generation, event),
        Message::Wheel(_) if s.file_details.is_some() => Task::none(),
        Message::ApplicationsLoaded { generation, result } => s.applications_loaded(generation, result),
        Message::OpenWith { generation, event } => s.open_with_event(generation, event),
        Message::ShowAppearance
            if s.opening.is_none()
                && s.folder_settings.is_none()
                && s.file_details.is_none()
                && s.application_picker.is_none() =>
        {
            s.show_appearance(appearance::Tab::Colors)
        }
        Message::Ui(UiEvent::DesktopAppearance(dark)) => {
            s.system_dark = dark;
            Task::none()
        }
        Message::ShowSettings => s.toggle_settings(),
        Message::SettingsSelected(entry) => s.choose_setting(entry),
        Message::ShowFolders => s.toggle_folders(),
        Message::FolderShortcut(shortcut) => s.folder_shortcut(shortcut),
        Message::Folder { generation, event } => s.folder_event(generation, event),
        Message::ShowSearchStatus => s.toggle_search_status(),
        Message::CloseSearchStatus => s.close_search_status(),
        Message::SearchStatusLoaded { generation, result } => s.search_status_loaded(generation, result),
        Message::ContextQuery { generation, query } => {
            if generation == s.scope_generation {
                update(s, Message::Query(query))
            } else {
                Task::none()
            }
        }
        Message::Query(q) => query_changed(s, q),
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
            Task::batch([search, crate::input::focus(search_id())])
        }
        Message::ActionQuery(q) => {
            if let Some(panel) = &mut s.panel {
                panel.filter(q);
                timing::event("action_input");
            }
            Task::none()
        }
        Message::Files { generation, result } => files_loaded(s, generation, result),
        Message::Cmd(cmd) => command(s, cmd),
        Message::Wheel(dy) => wheel(s, dy),
        Message::Selected(target) => {
            if s.panel.is_some() {
                s.clicks = ClickTracker::default();
                s.panel = None;
                return crate::input::focus(search_id());
            }
            if s.cursor.select_target(&target, &s.items) {
                s.clicks.press(&target);
            } else {
                s.clicks = ClickTracker::default();
            }
            crate::input::focus(search_id())
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
            crate::input::focus(actions_id())
        }
        Message::ResultAction { .. }
        | Message::ToggleActionsButton { .. }
        | Message::PanelBack { .. }
        | Message::PanelRun { .. }
        | Message::PanelClicked { .. } => update_action_click(s, message),
        Message::EmptyBack { step, generation }
            if s.panel.is_none() && s.selected().is_none() && s.generation == generation && empty_back(s) == step =>
        {
            command(s, Command::Back)
        }
        Message::ApplicationOpened { id, result } => application_opened(s, id, result),
        Message::Opened(result) => finish_open(s, result, Some(Kind::File)),
        Message::LinkOpened(result) => finish_open(s, result, None),
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
        Message::FocusLost if s.opening.is_none() && !s.settings_form_open() && !s.clipboard_owner => {
            s.launch_task = None;
            iced::exit()
        }
        // Layer-shell requests generated by `to_layer_message` are handled
        // by the runtime before they reach here.
        _ => Task::none(),
    }
}

fn update_action_click(s: &mut Launcher, message: Message) -> Task<Message> {
    let target = match &message {
        Message::ResultAction { target, .. }
        | Message::ToggleActionsButton { target, .. }
        | Message::PanelBack { target, .. }
        | Message::PanelRun { target, .. }
        | Message::PanelClicked { target, .. } => target,
        _ => return Task::none(),
    };
    // A filter or a late result can change rows after they were drawn.
    // Resolve the displayed identities instead of using the current row number.
    let Some(selected) = s.selected() else {
        return Task::none();
    };
    if !selected.target.same_identity(target) {
        return Task::none();
    }
    let actions = s.actions_for(&selected.target);
    match message {
        Message::ResultAction { action, query, .. }
            if s.panel.is_none() && query == s.query && actions.contains(&action) =>
        {
            s.perform(action)
        }
        Message::ToggleActionsButton { open: true, panel_id: None, .. } if s.panel.is_none() => {
            command(s, Command::ToggleActions)
        }
        Message::ToggleActionsButton { open: false, panel_id: Some(panel_id), .. }
            if s.panel.as_ref().is_some_and(|p| p.id == panel_id) =>
        {
            command(s, Command::ToggleActions)
        }
        Message::PanelBack { filtering, panel_id, filter_revision, .. }
            if s.panel.as_ref().is_some_and(|p| {
                p.id == panel_id && p.filter_revision == filter_revision && p.query.is_empty() != filtering
            }) =>
        {
            command(s, Command::Back)
        }
        Message::PanelRun { action, panel_id, filter_revision, .. }
            if s.panel.as_ref().is_some_and(|p| {
                p.id == panel_id && p.filter_revision == filter_revision && p.filtered_actions().contains(&action)
            }) =>
        {
            s.perform(action)
        }
        Message::PanelClicked { action, .. }
            if s.panel.as_ref().is_some_and(|p| p.filtered_actions().contains(&action)) =>
        {
            s.perform(action)
        }
        _ => Task::none(),
    }
}

fn query_changed(s: &mut Launcher, q: String) -> Task<Message> {
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
    s.file_search_succeeded = false;
    s.folder_more = false;
    s.status = if file_query { Status::Searching } else { s.scripts.search_status(s.scope) };
    s.rebuild(false);
    if !file_query {
        timing::ready(s.generation, s.items.len(), s.files.len(), true);
    }
    let search = if file_query { s.request_files() } else { Task::none() };
    Task::batch([search, s.resize()])
}

fn files_loaded(s: &mut Launcher, generation: u64, result: Result<Option<Vec<FileHit>>, String>) -> Task<Message> {
    if generation != s.generation {
        return Task::none(); // a newer query is in flight
    }
    let success = result.is_ok();
    match result {
        Ok(Some(files)) => {
            s.files = files;
            s.file_search_succeeded = true;
            if matches!(s.status, Status::Searching | Status::Idle) {
                s.status = Status::Idle;
            }
        }
        Ok(None) => return Task::none(),
        Err(e) => {
            s.files.clear();
            s.file_search_succeeded = false;
            s.status =
                Status::Error(if s.search_folder.is_some() { e } else { format!("File search unavailable: {e}") });
        }
    }
    let had_panel = s.panel.is_some();
    s.rebuild(true);
    timing::ready(generation, s.items.len(), s.files.len(), success);
    let focus = if had_panel && s.panel.is_none() { crate::input::focus(search_id()) } else { Task::none() };
    Task::batch([s.resize(), focus, s.sync_preview()])
}

fn application_opened(s: &mut Launcher, id: String, result: Result<(), String>) -> Task<Message> {
    if s.opening.as_ref().is_none_or(|(pending, _)| *pending != id) {
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

fn finish_open(s: &mut Launcher, result: Result<(), RunError>, history: Option<Kind>) -> Task<Message> {
    match result {
        Ok(()) => {
            if let Some((id, query)) = s.opening.take() {
                s.launch_task = None;
                if let Some(kind) = history {
                    s.usage.record(kind, &id, &query, usage::now());
                }
                return iced::exit();
            }
            Task::none()
        }
        Err(e) => {
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
    }
}

fn update_appearance(s: &mut Launcher, message: Message) -> Task<Message> {
    match message {
        Message::Appearance { generation, event } if generation == s.appearance_generation => s.appearance_event(event),
        Message::Cmd(command) => {
            let (task, close) = s.appearance.as_mut().unwrap().command(command);
            s.finish_appearance(task, close)
        }
        Message::FolderShortcut(folders::Shortcut::Save) => s.appearance_event(appearance::Event::Save),
        Message::FolderShortcut(folders::Shortcut::Remove) => s.appearance_event(appearance::Event::Discard),
        Message::RefreshSearchStatus => s.appearance_event(appearance::Event::Reload),
        Message::ShowAppearance => s.appearance_event(appearance::Event::Back),
        Message::Ui(_)
        | Message::FocusLost
        | Message::Files { .. }
        | Message::PreviewLoaded { .. }
        | Message::DetailsLoaded { .. }
        | Message::ApplicationsLoaded { .. }
        | Message::SearchStatusLoaded { .. }
        | Message::Opened(_)
        | Message::ApplicationOpened { .. } => update_content(s, message),
        _ => Task::none(),
    }
}

// Queued search input must not act through a modal view. Each view retains
// its own entry points for closing, refreshing and asynchronous completion.
fn blocks_search_input(s: &Launcher, message: &Message) -> bool {
    let file_panel = s.application_picker.is_some() || s.file_details.is_some();
    match message {
        Message::Query(_)
        | Message::ContextQuery { .. }
        | Message::Scope(_)
        | Message::ActionQuery(_)
        | Message::FolderBack { .. }
        | Message::Selected(_)
        | Message::Clicked(_)
        | Message::ResultAction { .. }
        | Message::ToggleActionsButton { .. }
        | Message::EmptyBack { .. }
        | Message::ContextMenu(_)
        | Message::PanelBack { .. }
        | Message::PanelRun { .. }
        | Message::PanelClicked { .. } => s.search_suspended(),
        Message::ShowSearchStatus => file_panel || s.folder_settings.is_some(),
        Message::ShowFolders | Message::ResultsWheel(_) => file_panel,
        _ => false,
    }
}

fn refresh_current_view(s: &mut Launcher) -> Task<Message> {
    if s.search_folder.is_some() && !s.search_suspended() {
        return s.refresh_folder();
    }
    if s.preview_visible() && s.text_preview.is_some() {
        let effect = s.text_preview.as_mut().unwrap().event(preview::Event::Refresh);
        return s.preview_effect(effect);
    }
    if let Some(panel) = &mut s.file_details {
        let effect = panel.event(details::Event::Refresh);
        return s.details_effect(effect);
    }
    if s.application_picker.is_some() {
        return s.application_effect(open_with::Effect::Reload);
    }
    if let Some(panel) = &mut s.folder_settings {
        let (task, effect) = panel.update(folders::Event::Reload, s.backend.clone());
        return s.finish_folders(task, effect);
    }
    if s.search_status.is_some() {
        return s.refresh_search_status();
    }
    Task::none()
}

fn wheel(s: &mut Launcher, dy: f32) -> Task<Message> {
    s.clicks = ClickTracker::default();
    if let Some(panel) = &mut s.application_picker {
        if s.opening.is_none() {
            panel.wheel(if dy > 0.0 { -1 } else { 1 });
        }
        return Task::none();
    }
    if s.search_suspended() {
        return Task::none(); // modal scrollables handle their own wheel events
    }
    move_results(s, dy)
}

fn move_results(s: &mut Launcher, dy: f32) -> Task<Message> {
    if dy == 0.0 {
        return Task::none();
    }
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
    if let Some(selected) = s.settings {
        return s.settings_command(selected, cmd);
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
        let page = p.cursor.visible.saturating_sub(1).max(1) as isize;
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
                return crate::input::focus(actions_id());
            }
            Command::Back | Command::ToggleActions => {
                s.panel = None;
                timing::event("actions_closed");
                return crate::input::focus(search_id());
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
    let page = s.cursor.visible.saturating_sub(1).max(1) as isize;
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
                return crate::input::focus(actions_id());
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
            return Task::batch([clear, crate::input::focus(search_id())]);
        }
        Command::Back => {
            if s.query.is_empty() {
                s.launch_task = None;
                return iced::exit();
            }
            // Apply the clear synchronously so it cannot arrive in a later context.
            let clear = update(s, Message::Query(String::new()));
            return Task::batch([clear, crate::input::focus(search_id())]);
        }
    }
    Task::none()
}

// ---------------------------------------------------------------- view

fn view(s: &Launcher) -> Element<'_, Message> {
    crate::input::capture(
        crate::material::renderer::opaque_fallback(view_content(s), s.appearance_theme(false)),
        s.input_epoch,
        s.input_focus(),
        s.input_context(),
        s.form_focus_cycle(),
        s.shortcut_routing(),
    )
}

fn search_placeholder(s: &Launcher) -> &'static str {
    if s.search_folder.is_some() {
        return "Search this folder and subfolders…";
    }
    match (s.scope, s.window_width < NARROW_SEARCH_WIDTH) {
        (Scope::All, true) => "Search…",
        (Scope::Applications, true) => "Search apps…",
        (Scope::Files, true) => "Search files…",
        (Scope::All, false) => "Search apps, files and commands…",
        (Scope::Applications, false) => "Search applications…",
        (Scope::Files, false) => "Search indexed files…",
        (Scope::Commands, _) => "Search commands…",
    }
}

fn view_content(s: &Launcher) -> Element<'_, Message> {
    if let Some(secondary) = s.secondary_view() {
        return secondary;
    }
    let placeholder = search_placeholder(s);
    let scope_generation = s.scope_generation;
    let input = text_input(placeholder, &s.query)
        .id(search_id())
        .on_input(move |query| Message::ContextQuery { generation: scope_generation, query })
        .size(if s.search_folder.is_some() { 15 } else { 20 })
        .padding(padding::top(4).bottom(4).left(36).right(8))
        .style(field_style);
    let search_icon = svg(svg::Handle::from_memory(
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m15.5 15.5 4.5 4.5"/></g></svg>"#,
    ))
    .width(18)
    .height(18)
    .style(|theme: &Theme, _| svg::Style { color: Some(palette::DIM.resolve(theme)) });
    // A decorative overlay keeps the entire field clickable, including the lens.
    // Bound its layer so software redraws do not damage the rest of the surface.
    let input =
        stack![input, container(search_icon).height(Length::Fill).align_y(Vertical::Center).padding(padding::left(10))]
            .clip(true);
    let header: Element<'_, Message> = if let Some(folder) = &s.search_folder {
        let generation = s.scope_generation;
        let path = folder.path.as_deref().and_then(|path| path.to_str());
        let display = path.map_or_else(|| folder.id.clone(), model::home_relative);
        let location = widget::responsive(move |size| {
            widget::tooltip(
                text(label_for_width(&display, size.width, 12.0, true))
                    .size(12)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(palette::DIM.resolve(theme)),
                    })
                    .wrapping(text::Wrapping::None),
                container(text(path.unwrap_or(&folder.id)).size(12).wrapping(text::Wrapping::WordOrGlyph))
                    .padding(8)
                    .max_width(600)
                    .style(surface_style),
                widget::tooltip::Position::Bottom,
            )
            .into()
        })
        .height(Length::Shrink);
        column![
            row![
                button(hint("Alt+←", "Back")).on_press(Message::FolderBack { generation }).style(status_button_style),
                location,
                button(hint("Ctrl+R", "Refresh")).on_press(Message::RefreshSearchStatus).style(status_button_style)
            ]
            .spacing(6)
            .align_y(Vertical::Center),
            input
        ]
        .spacing(4)
        .into()
    } else {
        column![input, views::search_navigation(s)].spacing(8).into()
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
            let available = (s.window_width as f32 - PAD * 2.0 - 1.0).max(1.0);
            let preview_width = (available * 0.6).min((available - 320.0).max(available * 0.4));
            row![
                container(list).width(Length::Fill),
                container(space()).width(1).height(Length::Fill).style(|theme| container::Style {
                    background: Some(Background::Color(palette::BORDER.resolve(theme))),
                    ..Default::default()
                }),
                container(preview).width(preview_width)
            ]
            .into()
        } else {
            results(s)
        };
        body = body.push(
            container(results).height(ROW_HEIGHT * s.cursor.visible as f32 + PAD).padding([PAD / 2.0, 0.0]).style(
                |theme: &Theme| container::Style {
                    background: Some(Background::Color(Color { a: 0.025, ..theme.palette().text })),
                    border: Border { radius: 9.0.into(), ..Default::default() },
                    ..Default::default()
                },
            ),
        );
        body = body.push(space().height(1));
    }
    body = body.push(footer(s));

    let surface = container(body).padding(PAD).width(Length::Fill).height(Length::Fill).style(surface_style);

    crate::shortcuts::capture_numbered(match (&s.panel, s.selected()) {
        (Some(p), Some(item)) => stack![surface, action_panel(p, &item.target)].into(),
        _ => surface.into(),
    })
}

#[cfg(test)]
fn material_theme(glass: bool) -> Theme {
    appearance::theme(Preferences::default(), None, glass)
}

pub(crate) fn surface_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme.palette().background)),
        border: Border {
            color: palette::BORDER.resolve(theme),
            width: 1.0,
            radius: (crate::material::RADIUS as f32).into(),
        },
        text_color: Some(palette::TEXT.resolve(theme)),
        ..Default::default()
    }
}

pub(crate) fn page_header<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> container::Container<'a, M> {
    container(content).height(INPUT_HEIGHT).padding(padding::left(16).right(8)).align_y(Vertical::Center)
}

pub(crate) fn divider<'a, M: 'a>() -> Element<'a, M> {
    container(space::horizontal())
        .height(1)
        .width(Length::Fill)
        .style(|theme| container::Style {
            background: Some(Background::Color(palette::FAINT.resolve(theme))),
            ..Default::default()
        })
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
pub(crate) fn field_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(palette::FAINT.resolve(theme)),
        border: Border {
            color: match status {
                text_input::Status::Focused { .. } => palette::ACCENT.resolve(theme),
                text_input::Status::Hovered => Color { a: 0.32, ..palette::TEXT.resolve(theme) },
                _ => palette::BORDER.resolve(theme),
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: palette::DIM.resolve(theme),
        placeholder: palette::DIM.resolve(theme),
        value: palette::TEXT.resolve(theme),
        selection: palette::SELECTED.resolve(theme),
    }
}

pub(crate) fn status_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: match status {
            button::Status::Pressed => Some(Background::Color(palette::SELECTED.resolve(theme))),
            button::Status::Hovered => Some(Background::Color(palette::FAINT.resolve(theme))),
            _ => None,
        },
        text_color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            palette::TEXT
        } else {
            palette::DIM
        }
        .resolve(theme),
        border: Border { radius: 5.0.into(), ..Default::default() },
        ..Default::default()
    }
}

pub(crate) fn tab_style(
    theme: &Theme,
    status: button::Status,
    selected: bool,
    selection_fill: Option<palette::Role>,
) -> button::Style {
    button::Style {
        background: selection_fill
            .filter(|_| selected)
            .or_else(|| matches!(status, button::Status::Hovered | button::Status::Pressed).then_some(palette::FAINT))
            .map(|role| Background::Color(role.resolve(theme))),
        text_color: if selected { palette::TEXT.resolve(theme) } else { palette::DIM.resolve(theme) },
        border: Border { radius: 5.0.into(), ..Default::default() },
        ..Default::default()
    }
}

pub(crate) fn list_row_button_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    button::Style {
        background: if selected {
            Some(Background::Color(palette::SELECTED.resolve(theme)))
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Some(Background::Color(palette::FAINT.resolve(theme)))
        } else {
            None
        },
        text_color: palette::TEXT.resolve(theme),
        border: Border { radius: 9.0.into(), ..Default::default() },
        ..Default::default()
    }
}

fn recovery_button_style(theme: &Theme, status: button::Status) -> button::Style {
    // Accent text loses contrast over bright backdrops through glass.
    let foreground = if theme.palette().background.a < 1.0 { palette::TEXT } else { palette::ACCENT };
    button::Style { text_color: foreground.resolve(theme), ..status_button_style(theme, status) }
}

fn status_metric(value: u64, label: &'static str) -> Element<'static, Message> {
    container(
        column![
            text(value.to_string()).size(22),
            text(label).size(12).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::DIM.resolve(theme)),
            })
        ]
        .spacing(3),
    )
    .padding(12)
    .width(Length::Fill)
    .style(|theme| container::Style {
        background: Some(Background::Color(palette::FAINT.resolve(theme))),
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
        page_header(row![text("Search status").size(20), space::horizontal(), refresh].align_y(Vertical::Center));
    let mut content = column![].spacing(12).padding(16);
    match status {
        SearchStatusPanel::Loading => {
            content = content.push(text("Checking file search…").size(18));
            content =
                content.push(text("Connecting to the search service.").size(13).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }));
        }
        SearchStatusPanel::Error(error) => {
            content = content.push(text("Search status unavailable").size(18).style(move |theme: &iced::Theme| {
                iced::widget::text::Style { color: Some(palette::ERROR.resolve(theme)) }
            }));
            content = content.push(
                text("Refresh to try again. If this persists, update the launcher and search service together.")
                    .size(13),
            );
            content = content.push(text(error).size(12).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::DIM.resolve(theme)),
            }));
        }
        SearchStatusPanel::Ready(info) => {
            let (title, help) = index_summary(info);
            content = content.push(text(title).size(18));
            content = content.push(text(help).size(13).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::DIM.resolve(theme)),
            }));
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
                    text("Folder watch limit reached. Some changes may not be detected promptly.").size(13).style(
                        move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(palette::ERROR.resolve(theme)),
                        },
                    ),
                );
            }
            if info.errors > 0 {
                content = content.push(text(format!("{} indexing errors reported during this service session. Check folder access and the service log for details.", info.errors)).size(13).style(move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::ERROR.resolve(theme)), }));
            }
            content = content.push(text("Indexed locations").size(14));
            if info.roots.is_empty() {
                content = content.push(text("No indexed locations yet. Check the indexing configuration; locations may also be unavailable while the first scan starts.").size(13).style(move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)), }));
            } else {
                for root in &info.roots {
                    content =
                        content.push(text(model::home_relative(root)).size(13).wrapping(text::Wrapping::WordOrGlyph));
                }
            }
        }
    }
    let detail = scrollable(content).id(status_scroll_id()).height(Length::Fill);
    let footer = container(
        row![
            text("Refresh to check for changes").size(12).width(Length::Fill).style(move |theme: &iced::Theme| {
                iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
            }),
            footer_button(hint("Esc", "Back"), Some(Message::CloseSearchStatus)),
        ]
        .align_y(Vertical::Center),
    )
    .height(FOOTER_HEIGHT)
    .padding(padding::left(16).right(PAD))
    .align_y(Vertical::Center)
    .style(footer_style);
    container(column![header, divider(), detail, divider(), footer])
        .padding(PAD)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(surface_style)
        .into()
}

fn empty_folder_text(status: &Status, query: &str) -> (&'static str, &'static str) {
    match status {
        Status::Searching if query.is_empty() => ("Loading folder…", ""),
        Status::Searching => ("Searching…", ""),
        Status::Error(_) => ("Folder unavailable", "Refresh to retry, or return to your previous search."),
        _ if query.is_empty() => {
            ("No indexed items in this folder", "Refresh, or type at least 2 characters to search subfolders.")
        }
        _ if query.chars().count() < MIN_FILE_QUERY => {
            ("Search this folder and subfolders", "Type at least 2 characters to search indexed files here.")
        }
        _ => ("No matching indexed files", "Try another name or return to your previous search."),
    }
}

fn empty_results_text(s: &Launcher) -> (&'static str, &'static str) {
    let query = s.query.trim();
    if s.search_folder.is_some() {
        return empty_folder_text(&s.status, query);
    }
    let short = query.chars().count() < MIN_FILE_QUERY;
    match &s.status {
        Status::Searching => ("Searching…", ""),
        Status::CatalogError => ("Command catalog unavailable", "Check launcher-scripts.toml and reopen FerroSeek."),
        Status::Error(_) if s.scope == Scope::Files => {
            ("File search unavailable", "Switch to Applications to search installed apps.")
        }
        Status::Error(_) => {
            ("File search unavailable", "You can still search applications and commands, or calculate a result.")
        }
        _ if calc::is_explicit(query) => ("Calculator", "Try = (2 + 3) * 4 or = 10 ft in m."),
        _ if s.scope == Scope::Files && short => {
            ("Search indexed files", "Type at least 2 characters to search files.")
        }
        _ if s.scope == Scope::Applications => ("No matching applications", "Try a different application name."),
        _ if s.scope == Scope::Commands => ("No matching commands", "Try another command name or clear the search."),
        _ if query.is_empty() => ("Start typing to search", "Find an application, file or command."),
        _ if short => ("No matching applications", "Type at least 2 characters to search files."),
        _ => ("No results", "Try a shorter name or a different keyword."),
    }
}

fn results(s: &Launcher) -> Element<'_, Message> {
    if s.items.is_empty() {
        let (msg, help) = empty_results_text(s);
        let mut empty = column![
            text(msg).size(18),
            text(help).size(13).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::DIM.resolve(theme)),
            })
        ]
        .spacing(8)
        .align_x(Horizontal::Center);
        if s.scope.file_query(&s.query) && !matches!(s.status, Status::Searching) {
            empty = empty.push(
                container(
                    row![
                        button(text("Check search status").size(13))
                            .on_press(Message::ShowSearchStatus)
                            .style(recovery_button_style),
                        button(text("Choose folders").size(13))
                            .on_press(Message::ShowFolders)
                            .style(recovery_button_style),
                    ]
                    .spacing(8),
                )
                .padding(padding::top(4)),
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

pub(crate) fn selection_bar<'a, M: 'a>(selected: bool, height: f32) -> Element<'a, M> {
    let bar = container(space::vertical()).width(3).height(height).style(move |theme| container::Style {
        background: selected.then_some(Background::Color(palette::ACCENT.resolve(theme))),
        border: Border { radius: 1.5.into(), ..Default::default() },
        ..Default::default()
    });
    container(bar).padding(padding::left(4)).into()
}

fn result_row(s: &Launcher, i: usize, slot: usize) -> Element<'_, Message> {
    let item = &s.items[i];
    let selected = i == s.cursor.selected;
    let icon: Element<'_, Message> = match s.item_icons.get(i).cloned().flatten() {
        Some(Icon::Svg(h)) => svg(h).width(ICON_SIZE).height(ICON_SIZE).into(),
        Some(Icon::Raster(h)) => image(h).width(ICON_SIZE).height(ICON_SIZE).into(),
        None => space().width(ICON_SIZE).height(ICON_SIZE).into(),
    };
    let labels = widget::responsive(move |size| {
        let title = label_for_width(&item.title, size.width, 15.0, matches!(item.target, Target::File(_)));
        let mut labels = column![text(title).size(15).wrapping(text::Wrapping::None)].spacing(1);
        let subtitle = match (&s.search_folder, &item.target) {
            (Some(folder), Target::File(file)) => folder
                .path
                .as_ref()
                .and_then(|parent| file.path.as_ref()?.parent()?.strip_prefix(parent).ok()?.to_str())
                .unwrap_or(&item.subtitle),
            _ => &item.subtitle,
        };
        if !subtitle.is_empty() {
            labels = labels.push(
                text(label_for_width(subtitle, size.width, 12.0, matches!(item.target, Target::File(_))))
                    .size(12)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(palette::DIM.resolve(theme)),
                    })
                    .wrapping(text::Wrapping::None),
            );
        }
        labels.into()
    })
    .height(Length::Shrink);
    let mut content = row![icon, container(labels).width(Length::Fill).clip(true)];
    if s.is_favorite(&item.target) {
        content = content.push(text("★").size(13).style(move |theme: &iced::Theme| iced::widget::text::Style {
            color: Some(palette::ACCENT.resolve(theme)),
        }));
    }
    if let Some(tag) = views::result_tag(s, item) {
        content = content.push(tag);
    }
    content = content.push(numbered_slot(slot, selected, s.panel.is_none()));
    let content = content.spacing(12).align_y(Vertical::Center).padding(padding::left(9).right(12));
    // With the row's 4 px inset, 4 + 3 + 9 px keeps the icon on the 28 px gutter.
    // Keep the full navigation slot clickable around the inset selection.
    mouse_area(selectable_result_row(content, selected))
        .on_press(Message::Selected(item.target.clone()))
        .on_double_click(Message::Clicked(item.target.clone()))
        .on_right_press(Message::ContextMenu(item.target.clone()))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn selectable_result_row<'a, M: 'a>(content: impl Into<Element<'a, M>>, selected: bool) -> Element<'a, M> {
    let content = content.into();
    let bar = selection_bar(selected, ROW_HEIGHT / 2.0);
    let row = container(row![bar, content].align_y(Vertical::Center))
        .height(ROW_HEIGHT - 4.0)
        .width(Length::Fill)
        .align_y(Vertical::Center)
        .style(move |theme| container::Style {
            background: selected.then_some(Background::Color(palette::SELECTED.resolve(theme))),
            border: Border { radius: 9.0.into(), ..Default::default() },
            ..Default::default()
        });
    let hover = container(space()).width(Length::Fill).height(Length::Fill).style(move |theme| container::Style {
        background: (!selected).then_some(Background::Color(palette::FAINT.resolve(theme))),
        border: Border { radius: 9.0.into(), ..Default::default() },
        ..Default::default()
    });
    container(widget::hover(row, hover)).padding([2, 4]).height(ROW_HEIGHT).clip(true).into()
}

fn search_status_text(s: &Launcher) -> (String, palette::Role) {
    let query = s.query.trim();
    let listing_folder = s.search_folder.is_some() && query.is_empty();
    let calculating = s.search_folder.is_none() && calc::is_explicit(query);
    match &s.status {
        Status::Error(e) => (e.clone(), palette::ERROR),
        Status::CatalogError => ("Check launcher-scripts.toml".into(), palette::ERROR),
        Status::Notice(n) => (n.clone(), palette::ACCENT),
        Status::Searching if listing_folder => ("Loading folder…".into(), palette::DIM),
        Status::Searching => ("Searching…".into(), palette::DIM),
        Status::Idle if listing_folder => (
            if s.folder_more {
                format!("First {} indexed items · Type to search", s.items.len())
            } else {
                format!("{} indexed items", s.items.len())
            },
            palette::DIM,
        ),
        Status::Idle if calculating => ("Calculator".into(), palette::DIM),
        Status::Idle => (result_count_text(s), palette::DIM),
    }
}

fn result_count_text(s: &Launcher) -> String {
    if s.items.first().is_some_and(|item| item.section == model::Section::Fallback) {
        return "No local results".into();
    }
    match (s.items.len(), s.query.trim().is_empty(), s.scope) {
        (0, false, _) => "No results".into(),
        (0, true, _) => "FerroSeek".into(),
        (_, true, Scope::All | Scope::Applications | Scope::Files) => "Suggestions".into(),
        (1, _, _) => "1 result".into(),
        (n, _, _) => format!("{n} results"),
    }
}

fn footer(s: &Launcher) -> Element<'_, Message> {
    let compact = s.window_width < NARROW_SEARCH_WIDTH;
    let (message, color) =
        if s.unconfirmed_open { (UNCONFIRMED_OPEN.to_owned(), palette::ERROR) } else { search_status_text(s) };
    let mut controls = row![footer_status(message, color)]
        .spacing(if compact { 8 } else { 12 })
        .align_y(Vertical::Center)
        .padding(if compact { padding::left(8).right(8) } else { padding::left(16).right(10) });
    if !s.unconfirmed_open {
        controls = controls.push(search_hints(s, compact));
    }
    container(controls).height(FOOTER_HEIGHT).align_y(Vertical::Center).style(footer_style).into()
}

fn footer_status(message: String, color: palette::Role) -> Element<'static, Message> {
    widget::responsive(move |size| {
        let wrapping = text::Wrapping::WordOrGlyph;
        let available = iced::Size::new(size.width, f32::INFINITY);
        let label = fit_label(&message, false, |candidate| {
            let measured = measure_text(candidate, 12.0, available, wrapping);
            measured.width <= size.width && measured.height <= size.height
        });
        text(label)
            .size(12)
            .style(move |theme: &Theme| text::Style { color: Some(color.resolve(theme)) })
            .wrapping(wrapping)
            .into()
    })
    .height(Length::Shrink)
    .into()
}

fn search_hints(s: &Launcher, compact: bool) -> Element<'_, Message> {
    let shortcut = s.shortcuts.bindings.get(Shortcut::Actions).to_string();
    if let Some(panel) = &s.panel
        && let Some(item) = s.selected()
    {
        let close = search_hint(shortcut, "Close actions", compact, true);
        let panel_id = Some(panel.id);
        return keyed_footer_button(
            FooterKey::Toggle { target: item.target.clone(), open: false, panel_id },
            close,
            Message::ToggleActionsButton { target: item.target.clone(), open: false, panel_id },
        );
    }
    let Some(item) = s.selected() else {
        let step = empty_back(s);
        let generation = s.generation;
        return keyed_footer_button(
            FooterKey::Empty { step, generation },
            hint("Esc", step.label()),
            Message::EmptyBack { step, generation },
        );
    };
    let acts = s.actions_for(&item.target);
    let primary = search_hint("Enter", acts[0].label(), compact, true);
    let result_button = |label, action| {
        let query = s.query.clone();
        keyed_footer_button(
            FooterKey::Result { target: item.target.clone(), action, query: query.clone() },
            label,
            Message::ResultAction { target: item.target.clone(), action, query },
        )
    };
    let mut hints = row![result_button(primary, acts[0])].spacing(4).align_y(Vertical::Center);
    if s.window_width >= 640 {
        let browse = acts.contains(&Action::BrowseFolder);
        let omit_secondary = browse && s.search_folder.is_some() && s.window_width < 720;
        if !omit_secondary && let Some(action) = model::secondary_action(&acts) {
            hints = hints.push(result_button(hint("Ctrl+Enter", action.label()), action));
        }
        if browse {
            hints = hints.push(result_button(hint("Alt+→", "Browse here"), Action::BrowseFolder));
        }
    }
    let actions = search_hint(shortcut, "Actions", compact, false);
    hints
        .push(keyed_footer_button(
            FooterKey::Toggle { target: item.target.clone(), open: true, panel_id: None },
            actions,
            Message::ToggleActionsButton { target: item.target.clone(), open: true, panel_id: None },
        ))
        .into()
}

fn search_hint<'a>(
    shortcut: impl text::IntoFragment<'a>,
    label: &'static str,
    compact: bool,
    primary: bool,
) -> Element<'a, Message> {
    if compact {
        if primary { primary_label(label) } else { text(label).size(12).into() }
    } else if primary {
        primary_hint(shortcut, label)
    } else {
        hint(shortcut, label)
    }
}

#[derive(Clone)]
enum FooterKey {
    Result { target: Target, action: Action, query: String },
    Empty { step: EmptyBack, generation: u64 },
    Panel { target: Target, action: Option<Action>, filtering: bool, panel_id: u64, filter_revision: u64 },
    Toggle { target: Target, open: bool, panel_id: Option<u64> },
}

impl PartialEq for FooterKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Result { target: a, action: act_a, query: q_a },
                Self::Result { target: b, action: act_b, query: q_b },
            ) => a.same_identity(b) && act_a == act_b && q_a == q_b,
            (Self::Empty { step: s_a, generation: g_a }, Self::Empty { step: s_b, generation: g_b }) => {
                s_a == s_b && g_a == g_b
            }
            (
                Self::Panel { target: a, action: act_a, filtering: f_a, panel_id: id_a, filter_revision: r_a },
                Self::Panel { target: b, action: act_b, filtering: f_b, panel_id: id_b, filter_revision: r_b },
            ) => a.same_identity(b) && act_a == act_b && f_a == f_b && id_a == id_b && r_a == r_b,
            (
                Self::Toggle { target: a, open: o_a, panel_id: id_a },
                Self::Toggle { target: b, open: o_b, panel_id: id_b },
            ) => a.same_identity(b) && o_a == o_b && id_a == id_b,
            _ => false,
        }
    }
}

/// Resets child widget state (such as an in-flight pointer press) when the
/// captured key changes across redraws.
struct Keyed<'a, K, M> {
    key: K,
    child: Element<'a, M>,
}

impl<'a, K: Clone + PartialEq + 'static, M: 'a> Widget<M, Theme, iced::Renderer> for Keyed<'a, K, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<K>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(self.key.clone())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.child)]
    }

    fn diff(&self, tree: &mut Tree) {
        let current = tree.state.downcast_mut::<K>();
        if *current != self.key {
            *current = self.key.clone();
            tree.children = vec![Tree::new(&self.child)];
        } else {
            tree.children[0].diff(self.child.as_widget());
        }
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.child.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &widget_layout::Limits,
    ) -> widget_layout::Node {
        self.child.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

pub(crate) fn keyed_footer_button<'a, K: Clone + PartialEq + 'static, M: Clone + 'a>(
    key: K,
    content: impl Into<Element<'a, M>>,
    message: M,
) -> Element<'a, M> {
    Element::new(Keyed { key, child: footer_button(content, Some(message)) })
}

pub(crate) fn footer_button<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    message: Option<M>,
) -> Element<'a, M> {
    button(container(content).height(Length::Fill).align_y(Vertical::Center))
        .on_press_maybe(message)
        .padding([0, 6])
        .height(24)
        .style(status_button_style)
        .into()
}

pub(crate) fn footer_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::FAINT.resolve(theme))),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    }
}

pub(crate) fn hint<'a, M: 'a>(key: impl text::IntoFragment<'a>, label: &'a str) -> Element<'a, M> {
    row![
        keycap(key, true),
        text(label)
            .size(12)
            .style(move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }),
    ]
    .spacing(6)
    .align_y(Vertical::Center)
    .into()
}

const PRIMARY_FONT: Font = Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT };

pub(crate) fn primary_label<'a, M: 'a>(label: &'a str) -> Element<'a, M> {
    text(label)
        .size(12)
        .font(PRIMARY_FONT)
        .style(|theme: &Theme| text::Style { color: Some(palette::TEXT.resolve(theme)) })
        .into()
}

pub(crate) fn primary_hint<'a, M: 'a>(key: impl text::IntoFragment<'a>, label: &'a str) -> Element<'a, M> {
    row![keycap(key, true), primary_label(label)].spacing(6).align_y(Vertical::Center).into()
}

fn keycap<'a, M: 'a>(key: impl text::IntoFragment<'a>, prominent: bool) -> Element<'a, M> {
    container(text(key).size(KEYCAP_SIZE).wrapping(text::Wrapping::None).style(move |theme: &iced::Theme| {
        iced::widget::text::Style { color: Some(if prominent { palette::TEXT } else { palette::DIM }.resolve(theme)) }
    }))
    .padding([1.0, KEYCAP_INSET])
    .style(move |theme| container::Style {
        background: prominent.then_some(Background::Color(palette::FAINT.resolve(theme))),
        border: Border {
            radius: 4.0.into(),
            color: if prominent { palette::BORDER.resolve(theme) } else { Color::TRANSPARENT },
            width: 1.0,
        },
        ..Default::default()
    })
    .into()
}

pub(crate) fn numbered_hint<M: 'static>(slot: usize, selected: bool) -> Element<'static, M> {
    numbered_slot(slot, selected, true)
}

fn numbered_slot<M: 'static>(slot: usize, selected: bool, available: bool) -> Element<'static, M> {
    if slot >= model::NUMBERED_SLOTS {
        return space().into();
    }
    let label = format!("Ctrl+{}", slot + 1);
    if available {
        keycap(label, selected)
    } else {
        // Action shortcuts take over without changing the result's text width.
        space().width(text_width(&label, KEYCAP_SIZE) + 2.0 * KEYCAP_INSET).into()
    }
}

fn action_panel<'a>(p: &'a Panel, target: &Target) -> Element<'a, Message> {
    let input = text_input("Search actions…", &p.query)
        .id(actions_id())
        .on_input(Message::ActionQuery)
        .size(14)
        .padding([8, 12])
        .style(field_style);
    // Title, field, rows and footer share a 12 px text inset.
    let file_title = matches!(target, Target::File(_));
    let title = container(
        widget::responsive(move |size| {
            let title =
                text(label_for_width(&p.title, size.width, 15.0, file_title)).size(15).wrapping(text::Wrapping::None);
            let mut label = column![title].spacing(2);
            if !p.context.is_empty() {
                label = label.push(
                    text(label_for_width(&p.context, size.width, 12.0, file_title))
                        .size(12)
                        .wrapping(text::Wrapping::None)
                        .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
                );
            }
            label.into()
        })
        .height(Length::Shrink),
    )
    .padding([0, 12]);
    let header = column![title, input].spacing(8);
    let actions = p.filtered_actions();
    let list = action_list(p, target, &actions);
    let footer = action_footer(p, target, &actions);
    let col = column![header, list, footer].spacing(8);
    let panel = container(col).width(360).padding(8).style(|theme| container::Style {
        background: Some(Background::Color(palette::RAISED.resolve(theme))),
        border: Border { color: palette::BORDER.resolve(theme), width: 1.0, radius: 10.0.into() },
        text_color: Some(palette::TEXT.resolve(theme)),
        // No shadow: iced_tiny_skia 0.14 paints it unclipped on partial redraws
        // (cursor blink), darkening the panel. RAISED and the border lift it.
        ..Default::default()
    });
    let panel = mouse_area(panel)
        // A click between commands keeps the panel and restores its field focus.
        .on_press(Message::Cmd(Command::OpenActions))
        .on_scroll(|delta| {
            let y = match delta {
                ScrollDelta::Lines { y, .. } => y,
                ScrollDelta::Pixels { y, .. } => y / ROW_HEIGHT,
            };
            Message::Wheel(y)
        });
    container(widget::opaque(panel))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Right)
        .align_y(Vertical::Bottom)
        .padding([FOOTER_HEIGHT + PAD + 4.0, PAD + 8.0])
        .into()
}

fn action_footer(p: &Panel, target: &Target, actions: &[Action]) -> Element<'static, Message> {
    let range = (actions.len() > p.cursor.visible).then(|| {
        let window = p.cursor.window(actions.len());
        format!("{}–{} / {}", window.start + 1, window.end, actions.len())
    });
    let action = actions.get(p.cursor.selected).copied();
    let filtering = !p.query.is_empty();
    let panel_id = p.id;
    let filter_revision = p.filter_revision;
    let target = target.clone();
    widget::responsive(move |size| {
        let mut run = row![].spacing(12).align_y(Vertical::Center);
        // Keep both controls readable when the list fits only one row.
        if let Some(range) = range.as_ref().filter(|_| size.width >= 288.0) {
            run = run.push(
                text(range.clone())
                    .size(11)
                    .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
            );
        }
        if let Some(action) = action {
            run = run.push(keyed_footer_button(
                FooterKey::Panel {
                    target: target.clone(),
                    action: Some(action),
                    filtering: false,
                    panel_id,
                    filter_revision,
                },
                primary_hint("Enter", "Run"),
                Message::PanelRun { target: target.clone(), action, panel_id, filter_revision },
            ));
        }
        let back = keyed_footer_button(
            FooterKey::Panel { target: target.clone(), action: None, filtering, panel_id, filter_revision },
            hint("Esc", if filtering { "Clear filter" } else { "Close actions" }),
            Message::PanelBack { target: target.clone(), filtering, panel_id, filter_revision },
        );
        row![back, space::horizontal(), run].align_y(Vertical::Center).padding([2, 6]).into()
    })
    .height(Length::Shrink)
    .into()
}

fn action_list<'a>(p: &'a Panel, target: &Target, actions: &[Action]) -> Element<'a, Message> {
    let mut list = column![].spacing(ACTION_ROW_GAP);
    if actions.is_empty() {
        list = list.push(
            container(
                column![
                    text("No matching actions").size(13),
                    text("Try another action name.")
                        .size(12)
                        .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
                ]
                .spacing(4),
            )
            .padding(padding::top(4).left(12).right(12)),
        );
    }
    for (slot, i) in p.cursor.window(actions.len()).enumerate() {
        let a = actions[i];
        let selected = i == p.cursor.selected;
        let label = widget::responsive(move |size| {
            text(label_for_width(p.label(a), size.width, 14.0, false)).size(14).wrapping(text::Wrapping::None).into()
        })
        .height(Length::Shrink);
        let content = row![
            label,
            text(a.shortcut()).size(11).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::DIM.resolve(theme)),
            }),
            numbered_hint(slot, selected),
        ]
        .spacing(8)
        .align_y(Vertical::Center)
        .padding(padding::left(5).right(12));
        let r = container(row![selection_bar(selected, 18.0), content].align_y(Vertical::Center))
            .height(ACTION_ROW_HEIGHT)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(move |theme| container::Style {
                background: selected.then_some(Background::Color(palette::SELECTED.resolve(theme))),
                border: Border { radius: 6.0.into(), ..Default::default() },
                ..Default::default()
            });
        let hover = container(space()).width(Length::Fill).height(Length::Fill).style(move |theme| container::Style {
            background: (!selected).then_some(Background::Color(palette::FAINT.resolve(theme))),
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        });
        list = list.push(
            mouse_area(widget::hover(r, hover))
                .on_press(Message::PanelClicked { target: target.clone(), action: a })
                .interaction(mouse::Interaction::Pointer),
        );
    }
    // Reserve the unfiltered list's room so the focused field stays under the caret.
    let slots = p.actions.len().min(p.cursor.visible) as f32;
    let list_height = (slots * (ACTION_ROW_HEIGHT + ACTION_ROW_GAP) - ACTION_ROW_GAP).max(44.0);
    container(list).height(list_height).into()
}

pub(crate) fn text_width(content: &str, size: f32) -> f32 {
    measure_text(content, size, iced::Size::INFINITE, text::Wrapping::None).width
}

#[cfg(test)]
fn text_width_with_font(content: &str, size: f32, font: Font) -> f32 {
    measure_text_with_font(content, size, font, iced::Size::INFINITE, text::Wrapping::None).width
}

fn measure_text(content: &str, size: f32, bounds: iced::Size, wrapping: text::Wrapping) -> iced::Size {
    measure_text_with_font(content, size, iced::Font::DEFAULT, bounds, wrapping)
}

fn measure_text_with_font(
    content: &str,
    size: f32,
    font: Font,
    bounds: iced::Size,
    wrapping: text::Wrapping,
) -> iced::Size {
    use iced::advanced::text::{Paragraph, Renderer};

    // Measure the same font and shaping as the labels; character counts
    // cannot predict wide glyphs or the space left by icons, tags and the sidebar.
    <iced::Renderer as Renderer>::Paragraph::with_text(iced::advanced::text::Text {
        content,
        bounds,
        size: iced::Pixels(size),
        line_height: Default::default(),
        font,
        align_x: Default::default(),
        align_y: Vertical::Top,
        shaping: Default::default(),
        wrapping,
    })
    .min_bounds()
}

pub(crate) fn label_for_width(value: &str, width: f32, size: f32, middle: bool) -> String {
    fit_label(value, middle, |content| text_width(content, size) <= width)
}

fn fit_label(value: &str, middle: bool, fits: impl Fn(&str) -> bool) -> String {
    if fits(value) {
        return value.to_string();
    }
    let shorten = |count| if middle { ellipsize_middle(value, count) } else { ellipsize_end(value, count) };
    if !fits(&shorten(3)) {
        return if fits("…") { "…".into() } else { String::new() };
    }
    let mut low = 3;
    let mut high = value.chars().count();
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if fits(&shorten(mid)) {
            low = mid;
        } else {
            high = mid;
        }
    }
    shorten(low)
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
    if max >= 5
        && let Some((_, name)) = s.rsplit_once('/')
        && !name.is_empty()
    {
        let name_len = name.chars().count();
        if name_len > max - 3 {
            return format!("…/{}", ellipsize_middle(name, max - 2));
        }
        let budget = max - name_len - 2;
        let prefix: String = s.chars().take(budget).collect();
        let boundary = prefix.rfind('/').map_or(prefix.as_str(), |i| &prefix[..i + 1]);
        let path = format!("{boundary}…/{name}");
        if path.chars().count() * 4 >= max * 3 {
            return path;
        }
        // Long directory components still need enough text to distinguish siblings.
        let prefix = prefix
            .rfind(' ')
            .filter(|&i| prefix[..i].chars().count() * 4 >= budget * 3)
            .map_or(prefix.as_str(), |i| &prefix[..i]);
        return format!("{}…/{name}", prefix.trim_end());
    }
    let chars: Vec<_> = s.chars().collect();
    let keep = max - 1;
    let mut tail = keep - keep * 2 / 3;
    let end_start = n - tail;
    if !chars[end_start - 1].is_whitespace()
        && !chars[end_start].is_whitespace()
        && let Some(space) = chars[..end_start].iter().rposition(|c| c.is_whitespace())
    {
        let whole_words = n - space - 1;
        if whole_words <= keep * 2 / 3 {
            tail = whole_words;
        }
    }
    let head = keep - tail;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[n - tail..].iter().collect();
    let start = if !chars[head].is_whitespace() {
        start.rfind(' ').filter(|&i| start[..i].chars().count() >= 3).map_or(start.as_str(), |i| &start[..i])
    } else {
        &start
    };
    format!("{}…{}", start.trim_end(), end.trim_start())
}

#[cfg(test)]
mod tests;
