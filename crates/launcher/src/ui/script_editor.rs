// SPDX-License-Identifier: GPL-3.0-or-later
//! Register and edit local commands without running them.

use std::sync::Arc;

use iced::widget::operation::{self, RelativeOffset};
use iced::widget::{Id, button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Length, Task, Theme};

use super::{Launcher, Message, Status, divider, field_style, footer_style, hint, page_header, palette};
use super::{primary_hint, status_button_style, surface_style, update_content};
use crate::folders::Shortcut;
use crate::input::FocusCycle;
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{ClickTracker, Command, Target};
use crate::script_commands::{self, Catalog, Script};

const FIELDS: [Field; 5] = [Field::Title, Field::Executable, Field::Description, Field::Arguments, Field::Parameters];

#[derive(Debug, Clone, Copy)]
pub enum Field {
    Title,
    Executable,
    Description,
    Arguments,
    Parameters,
}

impl Field {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Title => ("Title", "Release check"),
            Self::Executable => ("Executable path", "/home/you/bin/release-check"),
            Self::Description => ("Description (optional)", "Print a local release summary"),
            Self::Arguments => ("Fixed arguments (optional)", "[\"--summary\"]"),
            Self::Parameters => ("Ask for values at run time (optional)", "[\"Project name\", \"Branch\"]"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Input(Field, String),
    Focus(Field),
    Secondary,
    Back,
    Save,
    Delete,
    Written { title: Option<String>, result: Result<Catalog, String> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Editing,
    Discard,
    Delete,
    Writing,
}

#[derive(Clone, PartialEq, Eq)]
struct Draft {
    values: [String; FIELDS.len()],
    confirm: bool,
}

impl Draft {
    fn new(script: Option<&Script>) -> Self {
        match script {
            Some(script) => Self {
                values: [
                    script.title.clone(),
                    script.executable.to_string_lossy().into_owned(),
                    script.description.clone(),
                    script_commands::format_arguments(&script.args),
                    script_commands::format_arguments(&script.parameters),
                ],
                confirm: script.confirm,
            },
            None => Self { values: Default::default(), confirm: true },
        }
    }

    fn script(&self) -> Result<Script, String> {
        let [title, executable, description, args, parameters] = &self.values;
        let script = Script {
            title: title.trim().to_owned(),
            executable: executable.into(),
            description: description.clone(),
            args: script_commands::parse_arguments(args)?,
            parameters: script_commands::parse_arguments(parameters)?,
            confirm: self.confirm,
        };
        script.validate()?;
        Ok(script)
    }
}

pub(super) struct Editor {
    original: Option<Arc<Script>>,
    initial: Draft,
    draft: Draft,
    generation: u64,
    mode: Mode,
    error: Option<String>,
    focus: Field,
    inputs: [Id; FIELDS.len()],
    contexts: [Id; 4],
    scroll: Id,
}

enum Effect {
    None,
    Close,
    Focus(Field),
    Write(Option<Script>),
}

impl Editor {
    fn new(original: Option<Arc<Script>>, generation: u64, error: Option<String>) -> Self {
        let draft = Draft::new(original.as_deref());
        Self {
            original,
            initial: draft.clone(),
            draft,
            generation,
            mode: Mode::Editing,
            error,
            focus: Field::Title,
            inputs: std::array::from_fn(|_| Id::unique()),
            contexts: std::array::from_fn(|_| Id::unique()),
            scroll: Id::unique(),
        }
    }

    pub(super) fn context(&self) -> Id {
        self.contexts[self.mode as usize].clone()
    }

    pub(super) fn focus_target(&self) -> Option<Id> {
        (self.mode == Mode::Editing).then(|| self.inputs[self.focus as usize].clone())
    }

    pub(super) fn focus_cycle(&self) -> Option<FocusCycle> {
        (self.mode == Mode::Editing).then(|| FocusCycle {
            fields: FIELDS
                .into_iter()
                .map(|field| (self.inputs[field as usize].clone(), self.message(Event::Focus(field))))
                .collect(),
            tab_only: false,
        })
    }

    fn message(&self, event: Event) -> Message {
        Message::ScriptEdit { generation: self.generation, event }
    }

    fn focus(&self, field: Field) -> Task<Message> {
        let y = field as usize as f32 / (FIELDS.len() - 1) as f32;
        Task::batch([
            crate::input::focus(self.inputs[field as usize].clone()),
            operation::snap_to(self.scroll.clone(), RelativeOffset { x: 0.0, y }),
        ])
    }

    fn event(&mut self, event: Event) -> Effect {
        match (self.mode, event) {
            (Mode::Editing, event) => self.edit(event),
            (Mode::Discard | Mode::Delete, Event::Back) => {
                self.mode = Mode::Editing;
                Effect::Focus(self.focus)
            }
            (Mode::Discard, Event::Secondary) => Effect::Close,
            (Mode::Delete, Event::Secondary) => Effect::Write(None),
            _ => Effect::None,
        }
    }

    fn edit(&mut self, event: Event) -> Effect {
        match event {
            Event::Input(field, value) => {
                self.draft.values[field as usize] = value;
                self.error = None;
                self.focus = field;
                Effect::None
            }
            Event::Secondary => {
                self.draft.confirm = !self.draft.confirm;
                Effect::Focus(self.focus)
            }
            Event::Focus(field) => {
                self.focus = field;
                Effect::Focus(field)
            }
            Event::Back if self.draft != self.initial => {
                self.mode = Mode::Discard;
                Effect::None
            }
            Event::Back => Effect::Close,
            Event::Delete if self.original.is_some() => {
                self.mode = Mode::Delete;
                Effect::None
            }
            Event::Save => match self.draft.script() {
                Ok(script) => Effect::Write(Some(script)),
                Err(error) => {
                    self.error = Some(error);
                    Effect::None
                }
            },
            _ => Effect::None,
        }
    }

    fn form(&self) -> Element<'_, Event> {
        let mut fields = column![].spacing(12);
        for field in FIELDS {
            let (label, placeholder) = field.labels();
            let input = text_input(placeholder, &self.draft.values[field as usize])
                .id(self.inputs[field as usize].clone())
                .padding([7, 8])
                .on_input(move |value| Event::Input(field, value))
                .style(field_style);
            fields = fields.push(column![text(label).size(13), input].spacing(4));
        }
        let confirmation = if self.draft.confirm { "Confirmation on" } else { "Confirmation off" };
        fields.push(control(hint("Ctrl+Enter", confirmation), Event::Secondary)).padding([8, 16]).into()
    }

    fn body(&self) -> Element<'_, Event> {
        let message = match self.mode {
            Mode::Editing => return self.form(),
            Mode::Discard => "Discard unsaved changes?".into(),
            Mode::Delete => format!("Delete “{}”?", self.original.as_ref().unwrap().title),
            Mode::Writing => "Saving command…".into(),
        };
        container(text(message).size(15)).padding([12, 16]).into()
    }

    fn controls(&self) -> Element<'_, Event> {
        let buttons = match self.mode {
            Mode::Editing => self.edit_controls(),
            Mode::Discard => confirm_controls("Discard"),
            Mode::Delete => confirm_controls("Delete"),
            Mode::Writing => row![],
        };
        container(buttons.spacing(4).align_y(iced::Center))
            .height(FOOTER_HEIGHT)
            .padding([0.0, PAD])
            .align_y(iced::Center)
            .style(footer_style)
            .into()
    }

    fn edit_controls(&self) -> iced::widget::Row<'_, Event> {
        let mut buttons = row![control(hint("Esc", "Back"), Event::Back)];
        if self.original.is_some() {
            buttons = buttons.push(control(hint("Ctrl+D", "Delete"), Event::Delete));
        }
        buttons.push(space::horizontal()).push(control(primary_hint("Ctrl+S", "Save"), Event::Save))
    }

    pub(super) fn view(&self) -> Element<'_, Message> {
        let title = if self.original.is_some() { "Edit local command" } else { "Create local command" };
        let header = page_header(text(title).size(20)).height(56);
        let body = scrollable(self.body()).id(self.scroll.clone()).height(Length::Fill);
        let error = self.error.as_deref().filter(|_| self.mode == Mode::Editing).unwrap_or("");
        let notice = container(
            text(error).size(12).style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) }),
        )
        .padding([4, 16]);
        let view: Element<'_, Event> = container(column![header, divider(), body, notice, divider(), self.controls()])
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into();
        let generation = self.generation;
        view.map(move |event| Message::ScriptEdit { generation, event })
    }
}

fn control<'a>(content: impl Into<Element<'a, Event>>, event: Event) -> Element<'a, Event> {
    button(content).on_press(event).padding([4, 6]).style(status_button_style).into()
}

fn confirm_controls(label: &'static str) -> iced::widget::Row<'static, Event> {
    row![
        control(hint("Esc", "Keep editing"), Event::Back),
        space::horizontal(),
        control(primary_hint("Ctrl+Enter", label), Event::Secondary),
    ]
}

impl Launcher {
    pub(super) fn open_script_editor(&mut self, original: Option<Arc<Script>>) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.scripts.generation += 1;
        self.scripts.editor = Some(Editor::new(original, self.scripts.generation, self.scripts.catalog.error.clone()));
        crate::timing::event("script_editor_opened");
        self.resize()
    }

    pub(super) fn update_script_editor(&mut self, message: Message) -> Task<Message> {
        let event = match message {
            Message::ScriptEdit { generation, event } if generation == self.scripts.generation => event,
            Message::Cmd(Command::Back) => Event::Back,
            Message::Cmd(Command::Secondary) => Event::Secondary,
            Message::FolderShortcut(Shortcut::Save) => Event::Save,
            Message::FolderShortcut(Shortcut::Remove) => Event::Delete,
            Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_) => {
                return update_content(self, message);
            }
            _ => return Task::none(),
        };
        if let Event::Written { title, result } = event {
            return self.script_written(title, result);
        }
        let editor = self.scripts.editor.as_mut().expect("active script editor");
        match editor.event(event) {
            Effect::None => Task::none(),
            Effect::Focus(field) => editor.focus(field),
            Effect::Close => {
                self.scripts.editor = None;
                self.return_to_parent()
            }
            Effect::Write(next) => self.write_script(next),
        }
    }

    fn write_script(&mut self, next: Option<Script>) -> Task<Message> {
        let editor = self.scripts.editor.as_mut().unwrap();
        editor.mode = Mode::Writing;
        let previous = editor.original.clone();
        let generation = editor.generation;
        let settings = self.settings_path.clone();
        let write = zruntime::unblock(move || {
            let title = next.as_ref().map(|script| script.title.clone());
            let result = script_commands::change(&settings, previous.as_deref(), next);
            Event::Written { title, result }
        });
        Task::perform(write, move |event| Message::ScriptEdit { generation, event })
    }

    fn script_written(&mut self, title: Option<String>, result: Result<Catalog, String>) -> Task<Message> {
        let editor = self.scripts.editor.as_mut().unwrap();
        if editor.mode != Mode::Writing {
            return Task::none();
        }
        self.scripts.catalog = match result {
            Ok(catalog) => catalog,
            Err(error) => {
                editor.mode = Mode::Editing;
                editor.error = Some(error);
                return editor.focus(editor.focus);
            }
        };
        self.scripts.editor = None;
        crate::timing::event("script_editor_written");
        if let Some(title) = title {
            self.select_saved_local(&title, |target| matches!(target, Target::Script(_)), "Command saved")
        } else {
            self.rebuild(true);
            self.status = Status::Notice("Command deleted".into());
            self.return_to_parent()
        }
    }
}
