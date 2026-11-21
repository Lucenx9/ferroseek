// SPDX-License-Identifier: GPL-3.0-or-later
//! Editing a durable alias and resolving exact alias searches.

use iced::widget::{Id, button, column, container, responsive, row, space, text, text_input};
use iced::{Element, Length, Task, Theme};

use super::{
    Launcher, Message, Scope, Status, divider, field_style, footer_style, hint, label_for_width, page_header, palette,
    primary_hint, status_button_style, surface_style, update_content,
};
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{self, ClickTracker, Command, Item, Target};
use crate::saved::{self, Alias, AliasTarget};
use crate::{folders, timing};

pub(super) const HEIGHT: u32 = 268;

#[derive(Debug, Clone)]
pub enum Event {
    Input(String),
    Save,
    Back,
    Remove,
    Written(Result<saved::Store, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Editing,
    Discard,
    Remove,
    Writing { removed: bool },
}

pub(super) struct Editor {
    target: AliasTarget,
    title: String,
    previous: Option<String>,
    draft: String,
    error: Option<String>,
    generation: u64,
    mode: Mode,
    input: Id,
    editing: Id,
    confirming: Id,
}

impl Editor {
    fn new(target: AliasTarget, title: String, previous: Option<String>, generation: u64) -> Self {
        let draft = previous.clone().unwrap_or_default();
        Self {
            target,
            title,
            previous,
            draft,
            error: None,
            generation,
            mode: Mode::Editing,
            input: Id::unique(),
            editing: Id::unique(),
            confirming: Id::unique(),
        }
    }

    pub(super) fn focus_target(&self) -> Option<Id> {
        (self.mode == Mode::Editing).then(|| self.input.clone())
    }

    pub(super) fn context(&self) -> Id {
        if self.mode == Mode::Editing { self.editing.clone() } else { self.confirming.clone() }
    }

    fn event(&mut self, event: Event) -> Effect {
        match (self.mode, event) {
            (Mode::Editing, event) => self.edit(event),
            (Mode::Discard | Mode::Remove, Event::Back) => {
                self.mode = Mode::Editing;
                Effect::None
            }
            (Mode::Discard, Event::Remove) => Effect::Close,
            (Mode::Remove, Event::Remove) => Effect::Write(None),
            _ => Effect::None,
        }
    }

    fn edit(&mut self, event: Event) -> Effect {
        match event {
            Event::Input(draft) => {
                self.draft = draft;
                self.error = None;
                Effect::None
            }
            Event::Save => match Alias::new(self.target.clone(), &self.draft) {
                Ok(alias) => Effect::Write(Some(alias.word)),
                Err(error) => {
                    self.error = Some(error);
                    Effect::None
                }
            },
            Event::Back if self.draft != self.previous.as_deref().unwrap_or_default() => {
                self.mode = Mode::Discard;
                Effect::None
            }
            Event::Back => Effect::Close,
            Event::Remove if self.previous.is_some() => {
                self.mode = Mode::Remove;
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn notice(&self) -> String {
        match self.mode {
            Mode::Editing => self.error.clone().unwrap_or_default(),
            Mode::Discard => "Discard unsaved changes?".into(),
            Mode::Remove => format!("Remove alias “{}”?", self.previous.as_deref().unwrap_or_default()),
            Mode::Writing { .. } => "Saving alias…".into(),
        }
    }

    fn footer(&self) -> Element<'_, Event> {
        let controls = responsive(move |size| self.footer_controls(size.width < 320.0)).height(FOOTER_HEIGHT);
        container(controls).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style).into()
    }

    fn footer_controls(&self, compact: bool) -> Element<'_, Event> {
        let label = |key, name| if compact { text(name).size(12).into() } else { hint(key, name) };
        let confirm = matches!(self.mode, Mode::Discard | Mode::Remove);
        let editing = self.mode == Mode::Editing;
        let back = button(label("Esc", if confirm { "Keep editing" } else { "Back" }))
            .on_press_maybe((editing || confirm).then_some(Event::Back))
            .padding([4, 6])
            .style(status_button_style);
        let accept = if confirm {
            button(label("Ctrl+D", if self.mode == Mode::Remove { "Remove" } else { "Discard" }))
                .on_press(Event::Remove)
        } else {
            let save = if compact { text("Save").size(12).into() } else { primary_hint("Enter", "Save") };
            button(save).on_press_maybe(editing.then_some(Event::Save))
        }
        .padding([4, 6])
        .style(status_button_style);
        let mut controls = row![back].spacing(4).align_y(iced::Center);
        if editing && self.previous.is_some() {
            controls = controls.push(
                button(label("Ctrl+D", "Remove")).on_press(Event::Remove).padding([4, 6]).style(status_button_style),
            );
        }
        container(controls.push(space::horizontal()).push(accept)).center_y(Length::Fill).into()
    }

    pub(super) fn view(&self) -> Element<'_, Message> {
        let generation = self.generation;
        self.content().map(move |event| Message::AliasEditor { generation, event })
    }

    fn content(&self) -> Element<'_, Event> {
        let target = responsive(|size| text(label_for_width(&self.title, size.width, 15.0, false)).size(15).into())
            .height(Length::Shrink);
        let mut field =
            text_input("Example: notes", &self.draft).id(self.input.clone()).padding([7, 8]).style(field_style);
        if self.mode == Mode::Editing {
            field = field.on_input(Event::Input);
        }
        let tone = if self.error.is_some() { palette::ERROR } else { palette::DIM };
        let content = column![
            target,
            column![text("Alias").size(13), field].spacing(4),
            text(self.notice()).size(13).style(move |theme: &Theme| text::Style { color: Some(tone.resolve(theme)) }),
        ]
        .spacing(12)
        .padding([10, 16]);
        let heading = if self.previous.is_some() { "Edit alias" } else { "Set alias" };
        container(column![
            page_header(text(heading).size(20)).height(56),
            divider(),
            content,
            space::vertical(),
            divider(),
            self.footer()
        ])
        .padding(PAD)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(surface_style)
        .into()
    }
}

enum Effect {
    None,
    Close,
    Write(Option<String>),
}

impl Launcher {
    pub(super) fn open_alias_editor(&mut self, item: &Item) -> Task<Message> {
        if self.search_suspended() || self.opening.is_some() {
            return Task::none();
        }
        let Some(target) = self.alias_target(&item.target) else { return Task::none() };
        let title = if matches!(item.target, Target::UnavailableAlias(_)) { &item.subtitle } else { &item.title };
        let previous = self.saved.aliases.iter().find(|alias| alias.target == target).map(|alias| alias.word.clone());
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.edit_generation += 1;
        let mut editor = Editor::new(target, title.clone(), previous, self.edit_generation);
        editor.error = self.saved.error.clone();
        self.alias_editor = Some(editor);
        timing::event("alias_editor_opened");
        self.resize()
    }

    fn alias_target(&self, target: &Target) -> Option<AliasTarget> {
        match target {
            Target::App(index) => self.apps.get(*index).map(|app| AliasTarget::Application(app.id.clone())),
            Target::Command(command) => Some(AliasTarget::Command(command.id().into())),
            Target::UnavailableAlias(alias) => Some(alias.target.clone()),
            // The saved command namespace accepts both built-in IDs and local definition keys.
            target => self.scripts.history_key(target).map(|key| AliasTarget::Command(key.into())),
        }
    }

    pub(super) fn update_alias_editor(&mut self, message: Message) -> Task<Message> {
        let event = match message {
            Message::AliasEditor { generation, event } if generation == self.edit_generation => event,
            Message::Cmd(Command::Back) => Event::Back,
            Message::Cmd(Command::Activate) | Message::FolderShortcut(folders::Shortcut::Save) => Event::Save,
            Message::FolderShortcut(folders::Shortcut::Remove) => Event::Remove,
            Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_) => {
                return update_content(self, message);
            }
            _ => return Task::none(),
        };
        if let Event::Written(result) = event {
            return self.alias_written(result);
        }
        let editor = self.alias_editor.as_mut().unwrap();
        match editor.event(event) {
            Effect::None => Task::none(),
            Effect::Close => self.close_alias_editor(),
            Effect::Write(word) => {
                let path = saved::Store::path(&self.settings_path);
                editor.write(path, word)
            }
        }
    }

    fn alias_written(&mut self, result: Result<saved::Store, String>) -> Task<Message> {
        let editor = self.alias_editor.as_mut().unwrap();
        let Mode::Writing { removed } = editor.mode else { return Task::none() };
        match result {
            Err(error) => {
                editor.error = Some(error);
                editor.mode = Mode::Editing;
                timing::event("alias_write_failed");
                Task::none()
            }
            Ok(saved) => {
                self.saved = saved;
                self.rebuild(true);
                self.status = Status::Notice(if removed { "Alias removed" } else { "Alias saved" }.into());
                timing::event("alias_written");
                self.close_alias_editor()
            }
        }
    }

    fn close_alias_editor(&mut self) -> Task<Message> {
        self.alias_editor = None;
        timing::event("alias_editor_closed");
        self.return_to_parent()
    }

    pub(super) fn merge_alias(&mut self) {
        if self.search_folder.is_some() {
            return;
        }
        let query = self.query.trim();
        let Some(alias) = self.saved.aliases.iter().find(|alias| alias.word.eq_ignore_ascii_case(query)) else {
            return;
        };
        let Some(item) = self.alias_item(alias) else { return };
        self.items.retain(|other| !other.target.same_identity(&item.target));
        self.items.insert(0, item);
    }

    fn alias_item(&self, alias: &Alias) -> Option<Item> {
        let matched = match (&alias.target, self.scope) {
            (AliasTarget::Application(id), Scope::All | Scope::Applications) => {
                self.apps.iter().position(|app| &app.id == id).map(|index| model::app_item(&self.apps, index))
            }
            (AliasTarget::Command(id), Scope::All | Scope::Commands) => {
                model::stored_command(&self.scripts.catalog, id)
            }
            _ => return None,
        };
        Some(match matched {
            Some(mut item) => {
                item.subtitle = format!("Alias · {}", alias.word);
                item
            }
            None => model::unavailable_alias(alias),
        })
    }
}

impl Editor {
    fn write(&mut self, path: std::path::PathBuf, word: Option<String>) -> Task<Message> {
        self.mode = Mode::Writing { removed: word.is_none() };
        self.error = None;
        let target = self.target.clone();
        let previous = self.previous.clone();
        let generation = self.generation;
        let save = async move {
            zruntime::unblock(move || saved::change_alias(&path, target, previous.as_deref(), word.as_deref())).await
        };
        Task::perform(save, move |result| Message::AliasEditor { generation, event: Event::Written(result) })
    }
}
