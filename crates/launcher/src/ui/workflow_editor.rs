// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit a workflow and choose its ordered commands.

use std::sync::Arc;

use iced::widget::operation::{self, AbsoluteOffset, RelativeOffset};
use iced::widget::{Id, button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Length, Task, Theme};

use super::{Launcher, Message, Status, divider, field_style, footer_style, hint, label_for_width, page_header};
use super::{palette, primary_hint, selection_bar, status_button_style, surface_style, tab_style, update_content};
use crate::folders::Shortcut;
use crate::input::FocusCycle;
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::matcher;
use crate::model::{ClickTracker, Command, Target};
use crate::script_commands::{self, Saved, Script, Workflow};

const MAX_STEPS: usize = 8;
const ROW_HEIGHT: f32 = 36.0;
const ROW_GAP: f32 = 4.0;

#[derive(Debug, Clone, Copy)]
pub enum Field {
    Title,
    Description,
}

#[derive(Debug, Clone)]
pub enum Event {
    Input(Field, String),
    Focus(Field),
    Details,
    Steps,
    Pick,
    Filter(String),
    Choose(usize),
    Select(usize),
    Navigate(isize),
    Move(isize),
    Remove,
    Save,
    Delete,
    Secondary,
    Back,
    Written { title: Option<String>, result: Result<Saved, String> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Details,
    Steps,
    Pick,
    Discard,
    Delete,
    Writing,
}

struct Picker {
    commands: Vec<Arc<Script>>,
    matches: Vec<usize>,
    query: String,
    current: usize,
    input: Id,
    scroll: Id,
}

impl Picker {
    fn new(commands: &[Arc<Script>]) -> Self {
        let mut picker = Self {
            commands: commands.to_vec(),
            matches: Vec::new(),
            query: String::new(),
            current: 0,
            input: Id::unique(),
            scroll: Id::unique(),
        };
        picker.filter(String::new());
        picker
    }

    fn filter(&mut self, query: String) {
        let tokens = matcher::tokens(&query);
        let mut matches: Vec<_> = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(index, script)| {
                if tokens.is_empty() {
                    return Some((index, 0));
                }
                let title = matcher::fold(&script.title);
                matcher::score(&tokens, &[(title.as_str(), 100)]).map(|score| (index, score))
            })
            .collect();
        matches.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
        self.matches = matches.into_iter().map(|(index, _)| index).collect();
        self.query = query;
        self.current = 0;
    }

    fn view(&self) -> Element<'_, Event> {
        let filter = text_input("Search local commands", &self.query)
            .id(self.input.clone())
            .on_input(Event::Filter)
            .padding([7, 8])
            .style(field_style);
        let mut rows = column![].spacing(ROW_GAP);
        for (index, &command) in self.matches.iter().enumerate() {
            rows = rows.push(list_button(
                self.commands[command].title.clone(),
                index == self.current,
                Event::Choose(command),
            ));
        }
        if self.matches.is_empty() {
            let hint = if self.commands.is_empty() { "Create a local command first." } else { "No matching commands." };
            rows = rows.push(text(hint).size(13));
        }
        column![filter, scrollable(rows).id(self.scroll.clone()).height(Length::Fill)]
            .spacing(8)
            .padding([8, 16])
            .into()
    }
}

pub(super) struct Editor {
    original: Option<Arc<Workflow>>,
    draft: Workflow,
    mode: Mode,
    previous_mode: Mode,
    generation: u64,
    focus: Field,
    inputs: [Id; 2],
    contexts: [Id; 6],
    details_scroll: Id,
    steps_scroll: Id,
    current: usize,
    picker: Picker,
    error: Option<String>,
}

enum Effect {
    None,
    Focus,
    Close,
    Write(Option<Workflow>),
    Scroll(Id, usize),
}

impl Editor {
    fn new(original: Option<Arc<Workflow>>, commands: &[Arc<Script>], generation: u64, error: Option<String>) -> Self {
        let draft = original.as_deref().cloned().unwrap_or(Workflow {
            title: String::new(),
            description: String::new(),
            steps: Vec::new(),
        });
        Self {
            original,
            draft,
            mode: Mode::Details,
            previous_mode: Mode::Details,
            generation,
            focus: Field::Title,
            inputs: [Id::unique(), Id::unique()],
            contexts: std::array::from_fn(|_| Id::unique()),
            details_scroll: Id::unique(),
            steps_scroll: Id::unique(),
            current: 0,
            picker: Picker::new(commands),
            error,
        }
    }

    pub(super) fn context(&self) -> Id {
        self.contexts[self.mode as usize].clone()
    }

    pub(super) fn focus_target(&self) -> Option<Id> {
        match self.mode {
            Mode::Details => Some(self.inputs[self.focus as usize].clone()),
            Mode::Pick => Some(self.picker.input.clone()),
            _ => None,
        }
    }

    pub(super) fn focus_cycle(&self) -> Option<FocusCycle> {
        (self.mode == Mode::Details).then(|| FocusCycle {
            fields: [Field::Title, Field::Description]
                .into_iter()
                .map(|field| (self.inputs[field as usize].clone(), self.message(Event::Focus(field))))
                .collect(),
            tab_only: true,
        })
    }

    fn message(&self, event: Event) -> Message {
        Message::WorkflowEdit { generation: self.generation, event }
    }

    fn apply_focus(&self) -> Task<Message> {
        let focus = self.focus_target().map_or_else(Task::none, crate::input::focus);
        if self.mode != Mode::Details {
            return focus;
        }
        let offset = match self.focus {
            Field::Title => RelativeOffset::START,
            Field::Description => RelativeOffset::END,
        };
        Task::batch([focus, operation::snap_to(self.details_scroll.clone(), offset)])
    }

    fn event(&mut self, event: Event) -> Effect {
        match self.mode {
            Mode::Details | Mode::Steps => self.edit(event),
            Mode::Pick => self.pick(event),
            Mode::Discard | Mode::Delete => self.confirm(event),
            Mode::Writing => Effect::None,
        }
    }

    fn edit(&mut self, event: Event) -> Effect {
        match event {
            Event::Input(field, value) => {
                match field {
                    Field::Title => self.draft.title = value,
                    Field::Description => self.draft.description = value,
                }
                self.focus = field;
            }
            Event::Focus(field) => {
                self.focus = field;
                return Effect::Focus;
            }
            Event::Details => {
                self.mode = Mode::Details;
                return Effect::Focus;
            }
            Event::Steps => {
                self.mode = Mode::Steps;
                return self.scroll_steps();
            }
            Event::Pick | Event::Secondary => return self.open_picker(),
            Event::Select(index) => {
                self.current = index.min(self.last_step());
                return self.scroll_steps();
            }
            Event::Navigate(direction) if self.mode == Mode::Steps => {
                self.current = shifted(self.current, direction, self.draft.steps.len());
                return self.scroll_steps();
            }
            Event::Move(direction) if self.mode == Mode::Steps => return self.move_step(direction),
            Event::Remove if self.mode == Mode::Steps => return self.remove_step(),
            Event::Back => return self.back(),
            Event::Delete if self.original.is_some() => {
                self.confirm_mode(Mode::Delete);
                return Effect::None;
            }
            Event::Save => return self.save(),
            _ => return Effect::None,
        }
        self.error = None;
        Effect::None
    }

    fn open_picker(&mut self) -> Effect {
        if self.draft.steps.len() >= MAX_STEPS {
            self.error = Some("A workflow can have up to eight steps.".into());
            return Effect::None;
        }
        self.error = None;
        self.picker.filter(String::new());
        self.mode = Mode::Pick;
        Effect::Focus
    }

    fn pick(&mut self, event: Event) -> Effect {
        match event {
            Event::Filter(query) => {
                self.picker.filter(query);
                Effect::Scroll(self.picker.scroll.clone(), 0)
            }
            Event::Navigate(direction) => {
                self.picker.current = shifted(self.picker.current, direction, self.picker.matches.len());
                Effect::Scroll(self.picker.scroll.clone(), self.picker.current)
            }
            Event::Choose(index) if self.picker.matches.contains(&index) => {
                self.draft.steps.push(self.picker.commands[index].title.clone());
                self.current = self.last_step();
                self.mode = Mode::Steps;
                self.scroll_steps()
            }
            Event::Back => {
                self.mode = Mode::Steps;
                self.scroll_steps()
            }
            _ => Effect::None,
        }
    }

    fn confirm(&mut self, event: Event) -> Effect {
        match event {
            Event::Back => {
                self.mode = self.previous_mode;
                if self.mode == Mode::Steps { self.scroll_steps() } else { Effect::Focus }
            }
            Event::Secondary if self.mode == Mode::Delete => Effect::Write(None),
            Event::Secondary => Effect::Close,
            _ => Effect::None,
        }
    }

    fn back(&mut self) -> Effect {
        let dirty = self.original.as_deref().map_or_else(
            || !self.draft.title.is_empty() || !self.draft.description.is_empty() || !self.draft.steps.is_empty(),
            |original| *original != self.draft,
        );
        if dirty {
            self.confirm_mode(Mode::Discard);
            Effect::None
        } else {
            Effect::Close
        }
    }

    fn confirm_mode(&mut self, mode: Mode) {
        self.previous_mode = self.mode;
        self.mode = mode;
        self.error = None;
    }

    fn save(&mut self) -> Effect {
        if self.draft.title.trim().is_empty() {
            self.error = Some("Enter a workflow title.".into());
            self.mode = Mode::Details;
            self.focus = Field::Title;
            return Effect::Focus;
        }
        if self.draft.steps.len() < 2 {
            self.error = Some("Add at least two steps.".into());
            self.mode = Mode::Steps;
            return self.scroll_steps();
        }
        Effect::Write(Some(Workflow { title: self.draft.title.trim().to_owned(), ..self.draft.clone() }))
    }

    fn move_step(&mut self, direction: isize) -> Effect {
        if !self.draft.steps.is_empty() {
            let next = shifted(self.current, direction, self.draft.steps.len());
            self.draft.steps.swap(self.current, next);
            self.current = next;
            self.error = None;
        }
        self.scroll_steps()
    }

    fn remove_step(&mut self) -> Effect {
        if self.current < self.draft.steps.len() {
            self.draft.steps.remove(self.current);
            self.current = self.current.min(self.last_step());
            self.error = None;
        }
        self.scroll_steps()
    }

    fn last_step(&self) -> usize {
        self.draft.steps.len().saturating_sub(1)
    }

    fn scroll_steps(&self) -> Effect {
        Effect::Scroll(self.steps_scroll.clone(), self.current)
    }

    fn details(&self) -> Element<'_, Event> {
        let field = |field, title, placeholder, value| {
            column![
                text(title).size(13),
                text_input(placeholder, value)
                    .id(self.inputs[field as usize].clone())
                    .on_input(move |value| Event::Input(field, value))
                    .padding([7, 8])
                    .style(field_style)
            ]
            .spacing(4)
        };
        column![
            field(Field::Title, "Title", "Prepare release", &self.draft.title),
            field(Field::Description, "Description (optional)", "Build and check the release", &self.draft.description),
        ]
        .spacing(12)
        .padding([8, 16])
        .into()
    }

    fn steps(&self) -> Element<'_, Event> {
        let mut rows = column![].spacing(ROW_GAP);
        for (index, title) in self.draft.steps.iter().enumerate() {
            rows =
                rows.push(list_button(format!("{}. {title}", index + 1), index == self.current, Event::Select(index)));
        }
        if self.draft.steps.is_empty() {
            rows = rows.push(text("Add commands in the order they should run.").size(13));
        }
        column![scrollable(rows).id(self.steps_scroll.clone()).height(Length::Fill), self.step_controls()]
            .spacing(6)
            .padding([6, 16])
            .into()
    }

    fn step_controls(&self) -> Element<'_, Event> {
        row![
            control(text("Move up").size(12), (self.current > 0).then_some(Event::Move(-1))),
            control(text("Move down").size(12), (self.current < self.last_step()).then_some(Event::Move(1))),
            control(text("Remove").size(12), (!self.draft.steps.is_empty()).then_some(Event::Remove)),
        ]
        .spacing(6)
        .into()
    }

    fn tabs(&self) -> Element<'_, Event> {
        iced::widget::responsive(move |size| self.tab_bar(size.width < 300.0)).height(34).into()
    }

    fn tab_bar(&self, compact: bool) -> Element<'_, Event> {
        let tab = |label, mode, event| {
            let current = self.mode == mode;
            button(text(label).size(13))
                .on_press(event)
                .padding([4, 8])
                .style(move |theme, status| tab_style(theme, status, current, Some(palette::SELECTED)))
        };
        let mut tabs = row![
            tab("Details", Mode::Details, Event::Details),
            tab("Steps", Mode::Steps, Event::Steps),
            space::horizontal()
        ]
        .spacing(4);
        if self.mode == Mode::Steps {
            let add = if compact { text("Add step").size(12).into() } else { hint("Ctrl+Enter", "Add step") };
            tabs = tabs.push(control(add, (self.draft.steps.len() < MAX_STEPS).then_some(Event::Pick)));
        }
        container(tabs.align_y(iced::Center)).padding([0, 16]).center_y(Length::Fill).into()
    }

    fn controls(&self) -> iced::widget::Row<'_, Event> {
        match self.mode {
            Mode::Details | Mode::Steps => self.edit_controls(),
            Mode::Pick => row![
                control(hint("Esc", "Back"), Some(Event::Back)),
                space::horizontal(),
                control(
                    primary_hint("Enter", "Add step"),
                    self.picker.matches.get(self.picker.current).copied().map(Event::Choose)
                ),
            ],
            Mode::Discard | Mode::Delete => {
                let label = if self.mode == Mode::Delete { "Delete" } else { "Discard" };
                row![
                    control(hint("Esc", "Keep editing"), Some(Event::Back)),
                    space::horizontal(),
                    control(primary_hint("Ctrl+Enter", label), Some(Event::Secondary))
                ]
            }
            Mode::Writing => row![],
        }
    }

    fn edit_controls(&self) -> iced::widget::Row<'_, Event> {
        let mut controls = row![control(hint("Esc", "Back"), Some(Event::Back))];
        if self.original.is_some() {
            controls = controls.push(control(hint("Ctrl+D", "Delete"), Some(Event::Delete)));
        }
        controls.push(space::horizontal()).push(control(primary_hint("Ctrl+S", "Save"), Some(Event::Save)))
    }

    fn body(&self) -> Element<'_, Event> {
        match self.mode {
            Mode::Details => {
                column![self.tabs(), scrollable(self.details()).id(self.details_scroll.clone()).height(Length::Fill)]
                    .into()
            }
            Mode::Steps => column![self.tabs(), self.steps()].height(Length::Fill).into(),
            Mode::Pick => self.picker.view(),
            Mode::Discard => container(text("Discard unsaved changes?").size(15)).padding([12, 16]).into(),
            Mode::Delete => container(text(format!("Delete “{}”?", self.original.as_ref().unwrap().title)).size(15))
                .padding([12, 16])
                .into(),
            Mode::Writing => container(text("Saving workflow…").size(15)).padding([12, 16]).into(),
        }
    }

    pub(super) fn view(&self) -> Element<'_, Message> {
        let title = match (self.mode, self.original.is_some()) {
            (Mode::Pick, _) => "Add a command",
            (_, true) => "Edit workflow",
            (_, false) => "Create workflow",
        };
        let header = page_header(text(title).size(20)).height(56);
        let notice = self.error.as_deref().map(|error| {
            container(
                text(error).size(12).style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) }),
            )
            .max_height(60)
            .clip(true)
            .padding([2, 16])
        });
        let footer = container(self.controls().spacing(4).align_y(iced::Center))
            .height(FOOTER_HEIGHT)
            .padding([0.0, PAD])
            .align_y(iced::Center)
            .style(footer_style);
        let mut content = column![header, divider(), container(self.body()).height(Length::Fill)];
        if let Some(notice) = notice {
            content = content.push(notice);
        }
        let view: Element<'_, Event> = container(content.push(divider()).push(footer))
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into();
        view.map(|event| self.message(event))
    }
}

fn shifted(index: usize, direction: isize, count: usize) -> usize {
    index.saturating_add_signed(direction).min(count.saturating_sub(1))
}

fn control<'a>(content: impl Into<Element<'a, Event>>, event: Option<Event>) -> Element<'a, Event> {
    button(content).on_press_maybe(event).padding([4, 6]).style(status_button_style).into()
}

fn list_button(label: String, selected: bool, event: Event) -> Element<'static, Event> {
    let title = iced::widget::responsive(move |size| {
        text(label_for_width(&label, size.width, 14.0, false)).size(14).wrapping(text::Wrapping::None).into()
    })
    .height(Length::Shrink);
    button(row![selection_bar(selected, 18.0), title].spacing(8).align_y(iced::Center))
        .on_press(event)
        .height(ROW_HEIGHT)
        .width(Length::Fill)
        .padding([6, 8])
        .style(move |theme, status| tab_style(theme, status, selected, Some(palette::SELECTED)))
        .into()
}

impl Launcher {
    pub(super) fn open_workflow_editor(&mut self, original: Option<Arc<Workflow>>) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.scripts.generation += 1;
        self.scripts.workflow_editor = Some(Editor::new(
            original,
            self.scripts.catalog.commands(),
            self.scripts.generation,
            self.scripts.catalog.error.clone(),
        ));
        crate::timing::event("workflow_editor_opened");
        self.resize()
    }

    pub(super) fn update_workflow_editor(&mut self, message: Message) -> Task<Message> {
        let editor = self.scripts.workflow_editor.as_ref().expect("active workflow editor");
        let event = match message {
            Message::WorkflowEdit { generation, event } if generation == editor.generation => event,
            Message::Cmd(command) => match editor.key(command) {
                Some(event) => event,
                None => return Task::none(),
            },
            Message::FolderShortcut(Shortcut::Save) => Event::Save,
            Message::FolderShortcut(Shortcut::Remove) => Event::Delete,
            Message::WorkflowRemoveStep => Event::Remove,
            Message::Files { .. } | Message::Ui(_) | Message::InputClipboard(_) => {
                return update_content(self, message);
            }
            _ => return Task::none(),
        };
        if let Event::Written { title, result } = event {
            return self.workflow_written(title, result);
        }
        let editor = self.scripts.workflow_editor.as_mut().unwrap();
        match editor.event(event) {
            Effect::None => Task::none(),
            Effect::Focus => editor.apply_focus(),
            Effect::Scroll(id, index) => {
                operation::scroll_to(id, AbsoluteOffset { x: 0.0, y: index as f32 * (ROW_HEIGHT + ROW_GAP) })
            }
            Effect::Close => {
                self.scripts.workflow_editor = None;
                self.return_to_parent()
            }
            Effect::Write(next) => self.write_workflow(next),
        }
    }

    fn write_workflow(&mut self, next: Option<Workflow>) -> Task<Message> {
        let editor = self.scripts.workflow_editor.as_mut().unwrap();
        if editor.mode != Mode::Delete {
            editor.previous_mode = editor.mode;
        }
        editor.mode = Mode::Writing;
        editor.error = None;
        let previous = editor.original.clone();
        let generation = editor.generation;
        let settings = self.settings_path.clone();
        let write = zruntime::unblock(move || {
            let title = next.as_ref().map(|workflow| workflow.title.clone());
            let result = script_commands::change_workflow(&settings, previous.as_deref(), next);
            Event::Written { title, result }
        });
        Task::perform(write, move |event| Message::WorkflowEdit { generation, event })
    }

    fn workflow_written(&mut self, title: Option<String>, result: Result<Saved, String>) -> Task<Message> {
        let editor = self.scripts.workflow_editor.as_mut().unwrap();
        if editor.mode != Mode::Writing {
            return Task::none();
        }
        let Saved { catalog, warning } = match result {
            Ok(saved) => saved,
            Err(error) => {
                editor.mode = editor.previous_mode;
                editor.error = Some(error);
                return editor.apply_focus();
            }
        };
        self.scripts.catalog = catalog;
        self.scripts.workflow_editor = None;
        crate::timing::event("workflow_editor_written");
        if let Some(title) = title {
            let notice = warning.as_deref().unwrap_or("Workflow saved");
            self.select_saved_local(&title, |target| matches!(target, Target::Workflow(_)), notice)
        } else {
            self.rebuild(true);
            self.status = Status::Notice(warning.unwrap_or_else(|| "Workflow deleted".into()));
            self.return_to_parent()
        }
    }
}

impl Editor {
    fn key(&self, command: Command) -> Option<Event> {
        match (self.mode, command) {
            (_, Command::Back) => Some(Event::Back),
            (Mode::Details, Command::NextScope | Command::PreviousScope) => Some(Event::Steps),
            (Mode::Steps, Command::NextScope | Command::PreviousScope) => Some(Event::Details),
            (Mode::Pick, Command::Activate) => self.picker.matches.get(self.picker.current).copied().map(Event::Choose),
            (Mode::Steps | Mode::Pick, Command::Up) => Some(Event::Navigate(-1)),
            (Mode::Steps | Mode::Pick, Command::Down) => Some(Event::Navigate(1)),
            (Mode::Steps, Command::PreviousSection) => Some(Event::Move(-1)),
            (Mode::Steps, Command::NextSection) => Some(Event::Move(1)),
            (_, Command::Secondary) => Some(Event::Secondary),
            _ => None,
        }
    }
}
