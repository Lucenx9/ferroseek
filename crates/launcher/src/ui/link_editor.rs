// SPDX-License-Identifier: GPL-3.0-or-later
//! Native editor and its transitions to and from command search.

use iced::widget::operation::{self, RelativeOffset};
use iced::widget::{Id, button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Length, Task};

use super::{Launcher, Message, Scope, Status, divider, field_style, footer_style, hint, page_header, primary_hint};
use super::{search_id, status_button_style, surface_style, update, update_content};
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::links::{self, Link};
use crate::model::{ClickTracker, Command, Target};
use crate::{folders, timing};

const FIELDS: [Field; 3] = [Field::Name, Field::Address, Field::Keyword];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Address,
    Keyword,
}

impl Field {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Name => ("Name", "Project notes"),
            Self::Address => ("Destination", "https://example.net or ~/Projects"),
            Self::Keyword => ("Keyword (optional)", "notes"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Input(Field, String),
    Focus(Field),
    Back,
    Save,
    Remove,
    Confirm,
    Written { link: Option<Link>, result: Result<Vec<Link>, String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Editing,
    Discard,
    Delete,
    Writing,
}

pub(super) struct Editor {
    original: Option<Link>,
    name: String,
    address: String,
    keyword: String,
    focus: Field,
    mode: Mode,
    error: Option<String>,
    generation: u64,
    inputs: [Id; 3],
    scroll: Id,
    contexts: [Id; 4],
}

impl Editor {
    fn new(original: Option<Link>, generation: u64, error: Option<String>) -> Self {
        let (name, address, keyword) = original
            .as_ref()
            .map(|link| (link.name.clone(), link.address.clone(), link.keyword.clone()))
            .unwrap_or_default();
        Self {
            original,
            name,
            address,
            keyword,
            focus: Field::Name,
            mode: Mode::Editing,
            error,
            generation,
            inputs: std::array::from_fn(|_| Id::unique()),
            scroll: Id::unique(),
            contexts: std::array::from_fn(|_| Id::unique()),
        }
    }

    fn input_id(&self, field: Field) -> Id {
        self.inputs[field as usize].clone()
    }

    fn scroll_id(&self) -> Id {
        self.scroll.clone()
    }

    pub(super) fn context(&self) -> Id {
        self.contexts[self.mode as usize].clone()
    }

    pub(super) fn focus_target(&self) -> Option<Id> {
        (self.mode == Mode::Editing).then(|| self.input_id(self.focus))
    }

    fn dirty(&self) -> bool {
        let original = self.original.as_ref();
        original.map_or("", |link| &link.name) != self.name
            || original.map_or("", |link| &link.address) != self.address
            || original.map_or("", |link| &link.keyword) != self.keyword
    }

    fn event(&mut self, event: Event) -> Effect {
        match (self.mode, event) {
            (Mode::Editing, event) => self.edit(event),
            (Mode::Delete | Mode::Discard, Event::Back) => {
                self.mode = Mode::Editing;
                Effect::Focus(self.focus)
            }
            (Mode::Discard, Event::Confirm) => Effect::Close,
            (Mode::Delete, Event::Confirm) => Effect::Write(None),
            _ => Effect::None,
        }
    }

    fn edit(&mut self, event: Event) -> Effect {
        match event {
            Event::Input(field, value) => {
                *match field {
                    Field::Name => &mut self.name,
                    Field::Address => &mut self.address,
                    Field::Keyword => &mut self.keyword,
                } = value;
                self.focus = field;
                self.error = None;
                Effect::None
            }
            Event::Focus(field) => {
                self.focus = field;
                Effect::Focus(field)
            }
            Event::Back if self.dirty() => {
                self.mode = Mode::Discard;
                Effect::None
            }
            Event::Back => Effect::Close,
            Event::Remove if self.original.is_some() => {
                self.mode = Mode::Delete;
                Effect::None
            }
            Event::Save => match self.draft() {
                Ok(link) => Effect::Write(Some(link)),
                Err(error) => {
                    self.error = Some(error);
                    Effect::None
                }
            },
            _ => Effect::None,
        }
    }

    pub(super) fn focus_cycle(&self) -> Option<crate::input::FocusCycle> {
        let generation = self.generation;
        (self.mode == Mode::Editing).then(|| {
            crate::input::FocusCycle(
                FIELDS.map(|field| {
                    (self.input_id(field), Message::LinkEditor { generation, event: Event::Focus(field) })
                }),
            )
        })
    }

    pub(super) fn draft(&self) -> Result<Link, String> {
        Link::new(&self.name, &self.address, &self.keyword)
    }

    fn focus(&self, field: Field) -> Task<Message> {
        let focus = crate::input::focus(self.input_id(field));
        let offset = RelativeOffset { x: 0.0, y: field as usize as f32 / (FIELDS.len() - 1) as f32 };
        Task::batch([focus, operation::snap_to(self.scroll_id(), offset)])
    }

    fn field<'a>(&'a self, field: Field, value: &'a str) -> Element<'a, Event> {
        let (title, placeholder) = field.labels();
        let mut input = text_input(placeholder, value).id(self.input_id(field)).padding([7, 8]).style(field_style);
        if self.mode == Mode::Editing {
            input = input.on_input(move |value| Event::Input(field, value));
        }
        column![text(title).size(13), input].spacing(4).into()
    }

    fn notice(&self) -> Element<'_, Event> {
        let message = match self.mode {
            Mode::Discard => "Discard unsaved changes?".into(),
            Mode::Delete => format!("Delete “{}”?", self.original.as_ref().unwrap().name),
            Mode::Writing => "Saving links…".into(),
            Mode::Editing => self.error.clone().unwrap_or_default(),
        };
        let role = if self.error.is_some() { super::palette::ERROR } else { super::palette::DIM };
        text(message).size(13).style(move |theme: &iced::Theme| text::Style { color: Some(role.resolve(theme)) }).into()
    }

    fn controls(&self) -> Element<'_, Event> {
        let pending = matches!(self.mode, Mode::Discard | Mode::Delete);
        let editing = self.mode == Mode::Editing;
        let back = button(hint("Esc", if pending { "Keep editing" } else { "Back" }))
            .on_press_maybe((self.mode != Mode::Writing).then_some(Event::Back))
            .padding([4, 6])
            .style(status_button_style);
        let apply = if pending {
            button(hint("Ctrl+D", if self.mode == Mode::Delete { "Delete" } else { "Discard" }))
                .on_press(Event::Confirm)
        } else {
            button(primary_hint("Enter", "Save")).on_press_maybe(editing.then_some(Event::Save))
        }
        .padding([4, 6])
        .style(status_button_style);
        let remove = button(hint("Ctrl+D", "Delete"))
            .on_press_maybe(editing.then_some(Event::Remove))
            .padding([4, 6])
            .style(status_button_style);
        let mut controls = row![back].spacing(4).align_y(iced::Center);
        if editing && self.original.is_some() {
            controls = controls.push(remove);
        }
        container(controls.push(space::horizontal()).push(apply))
            .height(FOOTER_HEIGHT)
            .padding([0.0, PAD])
            .align_y(iced::Center)
            .style(footer_style)
            .into()
    }

    fn view(&self) -> Element<'_, Event> {
        let title = if self.original.is_some() { "Edit link" } else { "Create link" };
        let form = column![
            self.field(Field::Name, &self.name),
            self.field(Field::Address, &self.address),
            self.field(Field::Keyword, &self.keyword),
            text("For a web search, give it a keyword and use ?q={query} in the URL.")
                .size(12)
                .style(|theme: &iced::Theme| text::Style { color: Some(super::palette::DIM.resolve(theme)) }),
        ]
        .spacing(12)
        .padding([8, 16]);
        let body = scrollable(form).id(self.scroll_id()).height(Length::Fill);
        let notice = container(self.notice()).padding([4, 16]);
        container(column![page_header(text(title).size(20)), divider(), body, notice, divider(), self.controls()])
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into()
    }
}

enum Effect {
    None,
    Focus(Field),
    Close,
    Write(Option<Link>),
}

impl Launcher {
    pub(super) fn open_link_editor(&mut self, original: Option<Link>) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.link_generation += 1;
        self.link_editor = Some(Editor::new(original, self.link_generation, self.links.error.clone()));
        timing::event("link_editor_opened");
        self.resize()
    }

    pub(super) fn link_editor_view<'a>(&self, editor: &'a Editor) -> Element<'a, Message> {
        let generation = editor.generation;
        editor.view().map(move |event| Message::LinkEditor { generation, event })
    }

    pub(super) fn update_link_editor(&mut self, message: Message) -> Task<Message> {
        let generation = self.link_generation;
        let event = match message {
            Message::LinkEditor { generation: current, event } if current == generation => event,
            Message::Cmd(Command::Back) => Event::Back,
            Message::Cmd(Command::Activate) | Message::FolderShortcut(folders::Shortcut::Save) => Event::Save,
            Message::FolderShortcut(folders::Shortcut::Remove) => {
                let editor = self.link_editor.as_ref().unwrap();
                if editor.mode == Mode::Editing { Event::Remove } else { Event::Confirm }
            }
            Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_) => {
                return update_content(self, message);
            }
            _ => return Task::none(),
        };
        self.link_event(event)
    }

    fn link_event(&mut self, event: Event) -> Task<Message> {
        let editor = self.link_editor.as_mut().unwrap();
        if let Event::Written { link, result } = event {
            return self.link_written(link, result);
        }
        match editor.event(event) {
            Effect::None => Task::none(),
            Effect::Focus(field) => editor.focus(field),
            Effect::Close => self.close_link_editor(),
            Effect::Write(link) => {
                editor.mode = Mode::Writing;
                editor.error = None;
                let old = editor.original.clone();
                let path = links::Store::path(&self.settings_path);
                let generation = editor.generation;
                let write = async move {
                    zruntime::unblock(move || {
                        let result = links::change(&path, old.as_ref(), link.clone());
                        Event::Written { link, result }
                    })
                    .await
                };
                Task::perform(write, move |event| Message::LinkEditor { generation, event })
            }
        }
    }

    fn close_link_editor(&mut self) -> Task<Message> {
        self.link_editor = None;
        timing::event("link_editor_closed");
        self.return_to_parent()
    }

    fn link_written(&mut self, link: Option<Link>, result: Result<Vec<Link>, String>) -> Task<Message> {
        let editor = self.link_editor.as_mut().unwrap();
        if editor.mode != Mode::Writing {
            return Task::none();
        }
        let items = match result {
            Ok(items) => items,
            Err(error) => {
                editor.mode = Mode::Editing;
                editor.error = Some(error);
                return editor.focus(editor.focus);
            }
        };
        self.links = links::Store { items, error: None };
        let close = self.close_link_editor();
        let changed = if let Some(link) = link {
            self.scope = Scope::Commands;
            let changed = update(self, Message::Query(String::new()));
            self.cursor.select_target(&Target::Link(link, String::new()), &self.items);
            self.status = Status::Notice("Link saved".into());
            changed
        } else {
            self.rebuild(true);
            self.status = Status::Notice("Link deleted".into());
            Task::none()
        };
        timing::event("link_written");
        Task::batch([close, changed, self.resize(), crate::input::focus(search_id())])
    }
}

impl Launcher {
    pub(super) fn merge_links(&mut self) {
        if self.search_folder.is_some() || crate::calc::is_explicit(&self.query) {
            return;
        }
        let all = match self.scope {
            Scope::All => true,
            Scope::Commands => false,
            _ => return,
        };
        let matches = links::matching(&self.links.items, &self.query, all)
            .into_iter()
            .map(|(link, terms)| crate::model::link_item(link, terms));
        let index = if self.query.trim().is_empty() { self.items.len() } else { 0 };
        self.items.splice(index..index, matches);
    }

    pub(super) fn open_saved_link(&mut self, link: &Link, terms: &str) -> Task<Message> {
        if self.opening.is_some() {
            return Task::none();
        }
        match link.open(terms) {
            Ok(links::Open::Prompt(query)) => self.resume_query(query),
            Ok(request) => self.submit_link(link, request),
            Err(error) => {
                self.status = Status::Error(error);
                self.resize()
            }
        }
    }

    fn submit_link(&mut self, link: &Link, request: links::Open) -> Task<Message> {
        use super::RunError;

        if self.unconfirmed_open {
            self.status = Status::Error(super::UNCONFIRMED_OPEN.into());
            return self.resize();
        }
        let backend = self.backend.clone();
        let activation = self.activation.clone();
        self.opening = Some((link.name.clone(), String::new()));
        self.status = Status::Notice("Opening link…".into());
        let open = async move {
            match request {
                links::Open::Indexed(uri) => backend.run(uri, "").await,
                links::Open::Web(uri) => {
                    let token = activation.token(String::new()).await;
                    zruntime::unblock(move || crate::launch::open_web(&uri, token.as_deref()))
                        .await
                        .map_err(|error| RunError::Failed(error.to_string()))
                }
                links::Open::Prompt(_) => unreachable!("prompts are handled before dispatch"),
            }
        };
        let (task, handle) = Task::perform(open, Message::LinkOpened).abortable();
        self.launch_task = Some(handle.abort_on_drop());
        Task::batch([task, self.resize()])
    }
}
