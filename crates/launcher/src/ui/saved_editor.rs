// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit saved launcher destinations and reusable text.

use iced::widget::operation::{self, RelativeOffset};
use iced::widget::{Id, button, column, container, row, scrollable, space, text, text_editor, text_input};
use iced::{Element, Length, Task};

use super::{
    Launcher, Message, Scope, Status, divider, field_style, footer_button, footer_style, hint, page_header,
    primary_hint,
};
use super::{search_id, status_button_style, surface_style, update, update_content};
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::links::{self, Link, LinkFields};
use crate::model::{ClickTracker, Command, Target};
use crate::snippets::{Fields as SnippetFields, MAX_BODY_BYTES, Snippet};
use crate::{folders, saved, timing};

const SNIPPET_HEIGHT: u32 = 440;
const FIELDS: [Field; 3] = [Field::Name, Field::Value, Field::Keyword];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Value,
    Keyword,
}

impl Field {
    fn labels(self, kind: Kind) -> (&'static str, &'static str) {
        match self {
            Self::Name => ("Name", "Project notes"),
            Self::Value if kind == Kind::Snippet => ("Text", "Text to copy"),
            Self::Value => ("Destination", "https://example.net or ~/Projects"),
            Self::Keyword => ("Keyword (optional)", "notes"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Link,
    Snippet,
}

impl Kind {
    fn title(self, editing: bool) -> &'static str {
        match (self, editing) {
            (Self::Link, false) => "Create link",
            (Self::Link, true) => "Edit link",
            (Self::Snippet, false) => "Create snippet",
            (Self::Snippet, true) => "Edit snippet",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Link => "Link",
            Self::Snippet => "Snippet",
        }
    }

    fn contains(self, target: &Target) -> bool {
        matches!((self, target), (Self::Link, Target::Link(..)) | (Self::Snippet, Target::Snippet(_)))
    }
}

#[derive(Clone)]
pub(super) enum Entry {
    Link(Link),
    Snippet(Snippet),
}

impl Entry {
    fn fields(&self) -> (&str, &str, &str) {
        match self {
            Self::Link(link) => (&link.name, &link.address, &link.keyword),
            Self::Snippet(snippet) => (&snippet.name, &snippet.body, &snippet.keyword),
        }
    }

    fn write(
        path: &std::path::Path,
        kind: Kind,
        old: Option<Self>,
        next: Option<Self>,
    ) -> Result<saved::Store, String> {
        match kind {
            Kind::Link => {
                saved::change_link(path, old.as_ref().and_then(Self::link), next.as_ref().and_then(Self::link).cloned())
            }
            Kind::Snippet => saved::change_snippet(
                path,
                old.as_ref().and_then(Self::snippet),
                next.as_ref().and_then(Self::snippet).cloned(),
            ),
        }
    }

    fn link(&self) -> Option<&Link> {
        match self {
            Self::Link(link) => Some(link),
            _ => None,
        }
    }

    fn snippet(&self) -> Option<&Snippet> {
        match self {
            Self::Snippet(snippet) => Some(snippet),
            _ => None,
        }
    }
}

enum Value {
    Address(String),
    Text(text_editor::Content),
}

impl Value {
    fn text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Address(address) => address.as_str().into(),
            Self::Text(content) => content.text().into(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Input(Field, String),
    ToggleFallback,
    Text(text_editor::Action),
    Focus(Field),
    Back,
    Save,
    Remove,
    Confirm,
    Written { name: Option<String>, result: Result<saved::Store, String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Editing,
    Discard,
    Delete,
    Writing,
}

pub(super) struct Editor {
    kind: Kind,
    original: Option<Entry>,
    name: String,
    value: Value,
    keyword: String,
    fallback: bool,
    focus: Field,
    mode: Mode,
    error: Option<String>,
    generation: u64,
    inputs: [Id; 3],
    scroll: Id,
    contexts: [Id; 4],
}

impl Editor {
    fn new(kind: Kind, original: Option<Entry>, generation: u64, error: Option<String>) -> Self {
        let (name, value, keyword) = original.as_ref().map(Entry::fields).unwrap_or_default();
        let name = name.to_owned();
        let keyword = keyword.to_owned();
        let fallback = original.as_ref().and_then(Entry::link).is_some_and(Link::uses_fallback);
        let value = match kind {
            Kind::Link => Value::Address(value.into()),
            Kind::Snippet => Value::Text(text_editor::Content::with_text(value)),
        };
        Self {
            kind,
            original,
            name,
            value,
            keyword,
            fallback,
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

    pub(super) fn dialog_height(&self) -> Option<u32> {
        (self.kind == Kind::Snippet).then_some(SNIPPET_HEIGHT)
    }

    pub(super) fn context(&self) -> Id {
        self.contexts[self.mode as usize].clone()
    }

    pub(super) fn focus_target(&self) -> Option<Id> {
        (self.mode == Mode::Editing).then(|| self.input_id(self.focus))
    }

    fn dirty(&self) -> bool {
        let original = self.original.as_ref().map(Entry::fields).unwrap_or_default();
        original != (self.name.as_str(), self.value.text().as_ref(), self.keyword.as_str())
            || self.fallback != self.original.as_ref().and_then(Entry::link).is_some_and(Link::uses_fallback)
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
            Event::ToggleFallback if self.show_fallback_control() => {
                self.fallback = !self.fallback;
                self.error = None;
                Effect::Focus(self.focus)
            }
            Event::Input(field, value) => {
                match (field, &mut self.value) {
                    (Field::Name, _) => self.name = value,
                    (Field::Value, Value::Address(address)) => *address = value,
                    (Field::Keyword, _) => self.keyword = value,
                    _ => {}
                }
                self.focus = field;
                self.error = None;
                Effect::None
            }
            Event::Text(action) => {
                self.edit_text(action);
                Effect::None
            }
            Event::Focus(field) => {
                self.focus = field;
                Effect::Focus(field)
            }
            Event::Back if self.dirty() => {
                self.error = None;
                self.mode = Mode::Discard;
                Effect::None
            }
            Event::Back => Effect::Close,
            Event::Remove if self.original.is_some() => {
                self.error = None;
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

    fn edit_text(&mut self, action: text_editor::Action) {
        let Value::Text(content) = &mut self.value else { return };
        if action.is_edit() {
            let original = content.text();
            let cursor = content.cursor();
            content.perform(action);
            if content.text().len() > MAX_BODY_BYTES {
                *content = text_editor::Content::with_text(&original);
                content.move_to(cursor);
                self.error = Some("Use at most 8 KiB of text.".into());
                return;
            }
            self.error = None;
        } else {
            content.perform(action);
        }
        self.focus = Field::Value;
    }

    pub(super) fn focus_cycle(&self) -> Option<crate::input::FocusCycle> {
        let generation = self.generation;
        (self.mode == Mode::Editing).then(|| crate::input::FocusCycle {
            fields: FIELDS
                .into_iter()
                .map(|field| (self.input_id(field), Message::SavedEditor { generation, event: Event::Focus(field) }))
                .collect(),
            tab_only: self.kind == Kind::Snippet,
        })
    }

    pub(super) fn draft(&self) -> Result<Entry, String> {
        match &self.value {
            Value::Address(address) => Link::new(LinkFields { name: &self.name, address, keyword: &self.keyword })
                .and_then(|link| link.with_fallback(self.fallback))
                .map(Entry::Link),
            Value::Text(content) => {
                Snippet::new(SnippetFields { name: &self.name, keyword: &self.keyword, body: &content.text() })
                    .map(Entry::Snippet)
            }
        }
    }

    fn focus(&self, field: Field) -> Task<Message> {
        let focus = crate::input::focus(self.input_id(field));
        let offset = RelativeOffset { x: 0.0, y: field as usize as f32 / (FIELDS.len() - 1) as f32 };
        Task::batch([focus, operation::snap_to(self.scroll_id(), offset)])
    }

    fn field<'a>(&'a self, field: Field, value: &'a str) -> Element<'a, Event> {
        let (title, placeholder) = field.labels(self.kind);
        let title = if field == Field::Keyword && self.show_fallback_control() { "Keyword" } else { title };
        let mut input = text_input(placeholder, value).id(self.input_id(field)).padding([7, 8]).style(field_style);
        if self.mode == Mode::Editing {
            input = input.on_input(move |value| Event::Input(field, value));
        }
        column![text(title).size(13), input].spacing(4).into()
    }

    fn value_field(&self) -> Element<'_, Event> {
        let content = match &self.value {
            Value::Address(address) => return self.field(Field::Value, address),
            Value::Text(content) => content,
        };
        let (title, placeholder) = Field::Value.labels(self.kind);
        let mut input = text_editor(content)
            .id(self.input_id(Field::Value))
            .placeholder(placeholder)
            .height(110)
            .padding(8)
            .style(multiline_style);
        if self.mode == Mode::Editing {
            input = input.on_action(Event::Text);
        }
        column![text(title).size(13), input].spacing(4).into()
    }

    fn notice(&self) -> Element<'_, Event> {
        let message = match self.mode {
            Mode::Discard => "Discard unsaved changes?".into(),
            Mode::Delete => format!("Delete “{}”?", self.original.as_ref().unwrap().fields().0),
            Mode::Writing => format!("Saving {}…", self.kind.noun().to_lowercase()),
            Mode::Editing => self.error.clone().unwrap_or_default(),
        };
        let role = if self.error.is_some() { super::palette::ERROR } else { super::palette::DIM };
        text(message).size(13).style(move |theme: &iced::Theme| text::Style { color: Some(role.resolve(theme)) }).into()
    }

    fn controls(&self) -> Element<'_, Event> {
        let pending = matches!(self.mode, Mode::Discard | Mode::Delete);
        let editing = self.mode == Mode::Editing;
        let back = footer_button(
            hint("Esc", if pending { "Keep editing" } else { "Back" }),
            (self.mode != Mode::Writing).then_some(Event::Back),
        );
        let apply = if pending {
            footer_button(
                hint("Ctrl+D", if self.mode == Mode::Delete { "Delete" } else { "Discard" }),
                Some(Event::Confirm),
            )
        } else {
            let key = if self.kind == Kind::Snippet { "Ctrl+S" } else { "Enter" };
            footer_button(primary_hint(key, "Save"), editing.then_some(Event::Save))
        };
        let remove = footer_button(hint("Ctrl+D", "Delete"), editing.then_some(Event::Remove));
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

    fn show_fallback_control(&self) -> bool {
        match &self.value {
            Value::Address(address) => self.fallback || address.contains(links::QUERY),
            Value::Text(_) => false,
        }
    }

    fn link_help(&self) -> Element<'_, Event> {
        let fallback = self.show_fallback_control();
        let help = if fallback {
            "Offer this search when All has no local results."
        } else {
            "For a web search, give it a keyword and use ?q={query} in the URL."
        };
        let caption = text(help)
            .size(12)
            .style(|theme: &iced::Theme| text::Style { color: Some(super::palette::DIM.resolve(theme)) });
        if !fallback {
            return caption.into();
        }
        let label = if self.fallback { "Fallback on" } else { "Fallback off" };
        let toggle = button(hint("Ctrl+Enter", label))
            .on_press_maybe((self.mode == Mode::Editing).then_some(Event::ToggleFallback))
            .padding([4, 6])
            .style(status_button_style);
        column![caption, toggle].spacing(6).into()
    }

    fn view(&self) -> Element<'_, Event> {
        let title = self.kind.title(self.original.is_some());
        let form =
            column![self.field(Field::Name, &self.name), self.value_field(), self.field(Field::Keyword, &self.keyword)];
        let form = if self.kind == Kind::Link { form.push(self.link_help()) } else { form };
        let form = form.spacing(12).padding([8, 16]);
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

fn multiline_style(theme: &iced::Theme, status: text_editor::Status) -> text_editor::Style {
    let status = match status {
        text_editor::Status::Focused { is_hovered } => text_input::Status::Focused { is_hovered },
        text_editor::Status::Hovered => text_input::Status::Hovered,
        _ => text_input::Status::Active,
    };
    let style = field_style(theme, status);
    text_editor::Style {
        background: style.background,
        border: style.border,
        placeholder: style.placeholder,
        value: style.value,
        selection: style.selection,
    }
}

enum Effect {
    None,
    Focus(Field),
    Close,
    Write(Option<Entry>),
}

impl Launcher {
    pub(super) fn open_saved_editor(&mut self, kind: Kind, original: Option<Entry>) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.edit_generation += 1;
        self.saved_editor = Some(Editor::new(kind, original, self.edit_generation, self.saved.error.clone()));
        timing::event(if kind == Kind::Link { "link_editor_opened" } else { "snippet_editor_opened" });
        self.resize()
    }

    pub(super) fn saved_editor_view<'a>(&self, editor: &'a Editor) -> Element<'a, Message> {
        let generation = editor.generation;
        editor.view().map(move |event| Message::SavedEditor { generation, event })
    }

    pub(super) fn update_saved_editor(&mut self, message: Message) -> Task<Message> {
        let generation = self.edit_generation;
        let event = match message {
            Message::SavedEditor { generation: current, event } if current == generation => event,
            Message::Cmd(Command::Back) => Event::Back,
            Message::Cmd(Command::Secondary) => Event::ToggleFallback,
            Message::Cmd(Command::Activate) if self.saved_editor.as_ref().unwrap().kind == Kind::Link => Event::Save,
            Message::FolderShortcut(folders::Shortcut::Save) => Event::Save,
            Message::FolderShortcut(folders::Shortcut::Remove) => {
                let editor = self.saved_editor.as_ref().unwrap();
                if editor.mode == Mode::Editing { Event::Remove } else { Event::Confirm }
            }
            Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_) => {
                return update_content(self, message);
            }
            _ => return Task::none(),
        };
        self.saved_event(event)
    }

    fn saved_event(&mut self, event: Event) -> Task<Message> {
        let editor = self.saved_editor.as_mut().unwrap();
        if let Event::Written { name, result } = event {
            return self.saved_written(name, result);
        }
        match editor.event(event) {
            Effect::None => Task::none(),
            Effect::Focus(field) => editor.focus(field),
            Effect::Close => self.close_saved_editor(),
            Effect::Write(next) => {
                editor.mode = Mode::Writing;
                editor.error = None;
                let kind = editor.kind;
                let old = editor.original.clone();
                let path = saved::Store::path(&self.settings_path);
                let generation = editor.generation;
                let write = async move {
                    zruntime::unblock(move || {
                        let name = next.as_ref().map(|entry| entry.fields().0.to_owned());
                        let result = Entry::write(&path, kind, old, next);
                        Event::Written { name, result }
                    })
                    .await
                };
                Task::perform(write, move |event| Message::SavedEditor { generation, event })
            }
        }
    }

    fn close_saved_editor(&mut self) -> Task<Message> {
        if let Some(editor) = self.saved_editor.take() {
            timing::event(if editor.kind == Kind::Link { "link_editor_closed" } else { "snippet_editor_closed" });
        }
        self.return_to_parent()
    }

    fn saved_written(&mut self, name: Option<String>, result: Result<saved::Store, String>) -> Task<Message> {
        let editor = self.saved_editor.as_mut().unwrap();
        if editor.mode != Mode::Writing {
            return Task::none();
        }
        self.saved = match result {
            Ok(saved) => saved,
            Err(error) => {
                if editor.kind == Kind::Snippet {
                    timing::event("snippet_write_failed");
                }
                editor.mode = Mode::Editing;
                editor.error = Some(error);
                return editor.focus(editor.focus);
            }
        };
        let kind = editor.kind;
        let removed = name.is_none().then(|| editor.original.clone()).flatten();
        let close = self.close_saved_editor();
        let changed = if let Some(name) = name {
            self.select_saved_entry(kind, &name)
        } else {
            self.saved_deleted(kind, removed)
        };
        timing::event(if kind == Kind::Link { "link_written" } else { "snippet_written" });
        Task::batch([close, changed, self.resize(), crate::input::focus(search_id())])
    }

    fn saved_deleted(&mut self, kind: Kind, removed: Option<Entry>) -> Task<Message> {
        let target = removed.map(|entry| match entry {
            Entry::Link(link) => Target::Link(link, String::new()),
            Entry::Snippet(snippet) => Target::Snippet(snippet),
        });
        self.status = match target.as_ref().and_then(|target| self.remove_deleted_favorite(target)) {
            Some(error) => Status::Error(format!("{} deleted. Could not update favorites: {error}", kind.noun())),
            None => Status::Notice(format!("{} deleted", kind.noun())),
        };
        self.rebuild(true);
        Task::none()
    }

    fn remove_deleted_favorite(&mut self, target: &Target) -> Option<std::io::Error> {
        let (kind, key) = target.favorite_key(&self.apps, &self.saved)?;
        if self.favorites.contains(kind, &key) { self.favorites.set(kind, &key, false).err() } else { None }
    }

    fn select_saved_entry(&mut self, kind: Kind, name: &str) -> Task<Message> {
        self.leave_folder_search();
        self.scope = Scope::Commands;
        let changed = update(self, Message::Query(String::new()));
        if let Some(index) = self.items.iter().position(|item| kind.contains(&item.target) && item.title == name) {
            self.cursor.select(index, self.items.len());
        }
        self.status = Status::Notice(format!("{} saved", kind.noun()));
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use text_editor::{Action, Edit};

    #[test]
    fn snippet_capacity_uses_the_edited_buffer_with_crlf() {
        let original = format!("{}\r\nb", "a".repeat(MAX_BODY_BYTES - 3));
        let snippet = Snippet::new(SnippetFields { name: "Snippet", body: &original, keyword: "" }).unwrap();
        let mut editor = Editor::new(Kind::Snippet, Some(Entry::Snippet(snippet)), 0, None);
        editor.edit_text(Action::SelectAll);
        let replacement = "x".repeat(MAX_BODY_BYTES);
        editor.edit_text(Action::Edit(Edit::Paste(Arc::new(replacement.clone()))));
        assert_eq!(editor.value.text(), replacement);
        assert!(editor.error.is_none());

        editor.edit_text(Action::Edit(Edit::Insert('é')));
        assert_eq!(editor.value.text(), replacement);
        assert!(editor.error.is_some());
    }
}
