// SPDX-License-Identifier: GPL-3.0-or-later
//! Confirmation, output and return to search for local commands.

use std::path::Path;
use std::sync::Arc;

use iced::futures::channel::mpsc;
use iced::widget::operation::{self, AbsoluteOffset, RelativeOffset};
use iced::widget::{Id, column, container, row, scrollable, space, text};
use iced::{Element, Font, Length, Task, Theme};

use super::script_prompt::Prompt;
use super::workflow_editor;
use super::{Launcher, Message, Status, divider, footer_button, footer_style, hint, label_for_width, page_header};
use super::{palette, primary_hint, search_id, surface_style, update, update_content};
use crate::backend::UiEvent;
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{Action, Command, Scope, Target};
use crate::script_commands::{Catalog, Completion, Job, Plan, Report, Script, Workflow};

#[derive(Default)]
pub(super) struct State {
    pub catalog: Catalog,
    pub panel: Option<Panel>,
    pub editor: Option<super::script_editor::Editor>,
    pub workflow_editor: Option<workflow_editor::Editor>,
    pub generation: u64,
}

impl State {
    pub fn is_open(&self) -> bool {
        self.panel.is_some() || self.editor.is_some() || self.workflow_editor.is_some()
    }

    pub fn focus_target(&self) -> Option<Id> {
        self.editor
            .as_ref()
            .and_then(super::script_editor::Editor::focus_target)
            .or_else(|| self.workflow_editor.as_ref().and_then(workflow_editor::Editor::focus_target))
            .or_else(|| self.panel.as_ref()?.prompt()?.focus_target())
    }

    pub fn input_context(&self) -> Option<Id> {
        self.editor
            .as_ref()
            .map(super::script_editor::Editor::context)
            .or_else(|| self.workflow_editor.as_ref().map(workflow_editor::Editor::context))
            .or_else(|| self.panel.as_ref()?.prompt()?.context())
    }

    pub fn focus_cycle(&self) -> Option<crate::input::FocusCycle> {
        self.editor
            .as_ref()
            .and_then(super::script_editor::Editor::focus_cycle)
            .or_else(|| self.workflow_editor.as_ref().and_then(workflow_editor::Editor::focus_cycle))
            .or_else(|| self.panel.as_ref()?.prompt()?.focus_cycle(self.generation))
    }

    pub fn editor_view(&self) -> Option<Element<'_, Message>> {
        self.workflow_editor.as_ref().map(workflow_editor::Editor::view)
    }

    pub fn load(settings: &Path) -> Self {
        let catalog = Catalog::load(settings);
        if let Some(error) = &catalog.error {
            log::warn!("{error}");
        }
        Self { catalog, ..Self::default() }
    }

    pub fn search_status(&self, scope: Scope) -> Status {
        if scope == Scope::Commands && self.catalog.error.is_some() {
            Status::Error("Check launcher-scripts.toml".into())
        } else {
            Status::Idle
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Back,
    Run,
    RunAgain,
    Copy,
    StepStarted(usize),
    Finished(Report),
    Input(usize, String),
    Focus(usize),
}

enum Stage {
    Confirm { prompt: Prompt, previous: Option<Arc<Report>> },
    Running { _job: Job, step: usize },
    Finished(Arc<Report>),
}

pub(super) struct Panel {
    plan: Arc<Plan>,
    stage: Stage,
    scroll: Id,
    viewport: Id,
    copied: bool,
}

impl Panel {
    fn new(plan: Plan) -> Self {
        let prompt = Prompt::new(plan.parameters.len());
        Self {
            plan: Arc::new(plan),
            stage: Stage::Confirm { prompt, previous: None },
            scroll: Id::unique(),
            viewport: Id::unique(),
            copied: false,
        }
    }

    pub(super) fn prompt(&self) -> Option<&Prompt> {
        match &self.stage {
            Stage::Confirm { prompt, .. } => Some(prompt),
            _ => None,
        }
    }

    fn primary(&self) -> Option<(&'static str, &'static str, Event)> {
        match &self.stage {
            Stage::Confirm { .. } => Some(("Ctrl+Enter", "Run", Event::Run)),
            Stage::Finished(report) if !report.output.is_empty() => {
                let label = if self.copied { "Copied output" } else { "Copy output" };
                Some(("Enter", label, Event::Copy))
            }
            _ => None,
        }
    }

    pub(super) fn view(&self, generation: u64) -> Element<'_, Message> {
        let title = self.plan.title.as_str();
        let header = page_header(
            iced::widget::responsive(move |size| {
                text(label_for_width(title, size.width, 20.0, false)).size(20).wrapping(text::Wrapping::None).into()
            })
            .height(Length::Shrink),
        )
        .height(56);
        let scroll = container(scrollable(self.body(generation)).id(self.scroll.clone()).height(Length::Fill))
            .id(self.viewport.clone());
        let controls =
            iced::widget::responsive(move |size| self.controls(generation, size.width < 360.0)).height(FOOTER_HEIGHT);
        let footer = container(controls).height(FOOTER_HEIGHT).padding([0.0, PAD]).style(footer_style);
        container(column![header, divider(), scroll, divider(), footer])
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into()
    }

    fn controls(&self, generation: u64, compact: bool) -> Element<'static, Message> {
        let crowded = compact && matches!(&self.stage, Stage::Finished(report) if !report.output.is_empty());
        let control = |label: &'static str, full, event| {
            let content = if crowded { text(label).size(12).into() } else { full };
            footer_button(content, Message::Script { generation, event })
        };
        let back = if matches!(self.stage, Stage::Running { .. }) { "Cancel" } else { "Back" };
        let mut actions =
            row![control(back, hint("Esc", back), Event::Back), space::horizontal()].align_y(iced::Center).spacing(4);
        if matches!(self.stage, Stage::Finished(_)) {
            actions = actions.push(control("Run again", hint("Ctrl+R", "Run again"), Event::RunAgain));
        }
        if let Some((key, label, action)) = self.primary() {
            actions = actions.push(control(label, primary_hint(key, label), action));
        }
        container(actions).center_y(Length::Fill).into()
    }

    fn body(&self, generation: u64) -> Element<'_, Message> {
        let body = match &self.stage {
            Stage::Confirm { prompt, .. } => self.confirmation(prompt, generation),
            Stage::Running { step, .. } => self.running(*step),
            Stage::Finished(report) => report_body(report),
        };
        body.spacing(8).padding(16).into()
    }

    fn confirmation<'a>(&'a self, prompt: &'a Prompt, generation: u64) -> iced::widget::Column<'a, Message> {
        let mut body = column![];
        if self.plan.confirm {
            body = body.push(self.run_preview());
        }
        body.push(prompt.view(&self.plan.parameters, generation))
    }

    fn run_preview(&self) -> Element<'_, Message> {
        if self.plan.is_workflow() {
            let mut steps = column![text("Run these commands in order?").size(15)].spacing(6);
            for (index, step) in self.plan.steps.iter().enumerate() {
                steps = steps.push(text(format!("{}. {}", index + 1, step.script.title)).size(13));
                if step.script.confirm {
                    steps = steps.push(output_text(command_preview(&step.script)));
                }
            }
            return steps.into();
        }
        let script = &self.plan.steps[0].script;
        column![text("Run this command?").size(15), output_text(command_preview(script))].spacing(8).into()
    }

    fn running(&self, index: usize) -> iced::widget::Column<'_, Message> {
        if self.plan.is_workflow() {
            let step = &self.plan.steps[index];
            column![
                text(format!("Step {} of {}", index + 1, self.plan.steps.len())).size(13),
                text(step.script.title.as_str()).size(15)
            ]
            .spacing(6)
        } else {
            column![text("Running…").size(15)]
        }
    }

    fn start(&mut self, inputs: Vec<String>, generation: u64) -> Task<Message> {
        let (tx, rx) = mpsc::unbounded();
        let (job, work) = Job::start(Arc::clone(&self.plan), inputs, move |index| {
            let _ = tx.unbounded_send(index);
        });
        self.stage = Stage::Running { _job: job, step: 0 };
        self.copied = false;
        crate::timing::event("script_started");
        Task::batch([
            self.scroll_start(),
            Task::run(rx, move |index| Message::Script { generation, event: Event::StepStarted(index) }),
            Task::perform(work, move |result| Message::Script { generation, event: Event::Finished(result) }),
        ])
    }

    fn scroll_start(&self) -> Task<Message> {
        operation::snap_to(self.scroll.clone(), RelativeOffset::START)
    }

    fn needs_review(&self) -> bool {
        self.plan.confirm || !self.plan.parameters.is_empty()
    }

    fn restore_report(&mut self) -> bool {
        if let Stage::Confirm { previous, .. } = &mut self.stage
            && let Some(report) = previous.take()
        {
            self.stage = Stage::Finished(report);
            return true;
        }
        false
    }

    fn command(&self, command: Command, generation: u64) -> Task<Message> {
        let id = self.scroll.clone();
        match command {
            Command::Activate => Task::done(Message::Script { generation, event: Event::Copy }),
            Command::Secondary => Task::done(Message::Script { generation, event: Event::Run }),
            Command::Up => operation::scroll_by(id, AbsoluteOffset { x: 0.0, y: -40.0 }),
            Command::Down => operation::scroll_by(id, AbsoluteOffset { x: 0.0, y: 40.0 }),
            Command::PageUp => operation::scroll_by(id, AbsoluteOffset { x: 0.0, y: -240.0 }),
            Command::PageDown => operation::scroll_by(id, AbsoluteOffset { x: 0.0, y: 240.0 }),
            Command::First => operation::snap_to(id, RelativeOffset::START),
            Command::Last => operation::snap_to(id, RelativeOffset::END),
            _ => Task::none(),
        }
    }
}

fn command_preview(script: &Script) -> String {
    let executable = script.executable.display();
    let args = script.args.iter().map(|arg| format!("{arg:?}")).collect::<Vec<_>>().join(" ");
    format!("{executable}\n{args}")
}

fn output_text<'a>(value: impl iced::widget::text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(value).font(Font::MONOSPACE).size(13).wrapping(text::Wrapping::WordOrGlyph)
}

fn error_text(value: &str) -> iced::widget::Text<'_> {
    text(value).size(15).style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) })
}

fn report_body(report: &Report) -> iced::widget::Column<'_, Message> {
    let summary = match &report.completion {
        Ok(Completion::Exited(status)) if status.success() => text("Completed").size(15),
        Ok(Completion::Exited(status)) => {
            let message = format!("Command failed ({status})");
            text(message).size(15).style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) })
        }
        Ok(Completion::Cancelled) => text("Cancelled").size(15),
        Ok(Completion::TimedOut) => error_text("Stopped after 30 seconds."),
        Err(error) => error_text(error),
    };
    let mut body = column![summary];
    if let Some(step) = &report.failed_step {
        body = body.push(text(step).size(13));
    }
    if report.truncated {
        body = body.push(text("Showing the first 64 KiB.").size(12));
    }
    if report.output.is_empty() {
        body.push(text("No output.").size(13))
    } else {
        body.push(output_text(report.output.as_str()))
    }
}

impl Launcher {
    pub(super) fn select_saved_local(
        &mut self,
        title: &str,
        matches: impl Fn(&Target) -> bool,
        notice: &str,
    ) -> Task<Message> {
        self.scope = Scope::Commands;
        let refresh = update(self, Message::Query(String::new()));
        if let Some(index) = self.items.iter().position(|item| matches(&item.target) && item.title == title) {
            self.cursor.select(index, self.items.len());
        }
        self.status = Status::Notice(notice.into());
        Task::batch([refresh, crate::input::focus(search_id())])
    }

    pub(super) fn activate_command(&mut self, target: &Target, action: Action) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        match (target, action) {
            (Target::Command(command), Action::OpenCommand) => self.open_builtin(*command),
            (Target::Script(script), Action::RunScript) => self.open_plan(Plan::single(script.clone())),
            (Target::Workflow(workflow), Action::RunScript) => self.open_workflow(workflow),
            (Target::Workflow(workflow), Action::EditWorkflow) => self.open_workflow_editor(Some(workflow.clone())),
            (Target::Script(script), Action::EditScript) => self.open_script_editor(Some(script.clone())),
            _ => Task::none(),
        }
    }

    fn open_workflow(&mut self, workflow: &Workflow) -> Task<Message> {
        match self.scripts.catalog.plan(workflow) {
            Ok(plan) => self.open_plan(plan),
            Err(error) => {
                self.panel = None;
                self.status = Status::Error(error);
                self.return_to_parent()
            }
        }
    }

    fn open_plan(&mut self, plan: Plan) -> Task<Message> {
        let panel = Panel::new(plan);
        let initial = if panel.needs_review() { Event::Focus(0) } else { Event::Run };
        self.panel = None;
        self.scripts.generation += 1;
        self.scripts.panel = Some(panel);
        let action = self.script_event(initial);
        Task::batch([self.resize(), action])
    }

    fn script_back(&mut self) -> Task<Message> {
        let panel = self.scripts.panel.as_mut().expect("active script view");
        if panel.restore_report() {
            return panel.scroll_start();
        }
        self.scripts.panel = None;
        self.return_to_parent()
    }

    fn repeat_script(&mut self, report: Arc<Report>) -> Task<Message> {
        let panel = self.scripts.panel.as_mut().expect("active script view");
        // Progress or input events already queued for the preceding run can still arrive.
        self.scripts.generation += 1;
        if !panel.needs_review() {
            return panel.start(Vec::new(), self.scripts.generation);
        }
        let mut prompt = Prompt::new(panel.plan.parameters.len());
        let focus = prompt.focus(0, &panel.scroll, &panel.viewport);
        panel.stage = Stage::Confirm { prompt, previous: Some(report) };
        panel.scroll_start().chain(focus)
    }

    pub(super) fn update_scripts(&mut self, message: Message) -> Task<Message> {
        if self.scripts.workflow_editor.is_some() {
            return self.update_workflow_editor(message);
        }
        if self.scripts.editor.is_some() {
            return self.update_script_editor(message);
        }
        match message {
            Message::Script { generation, event } if generation == self.scripts.generation => self.script_event(event),
            Message::RefreshSearchStatus => self.script_event(Event::RunAgain),
            Message::Cmd(Command::Back) => self.script_event(Event::Back),
            Message::Cmd(command) => self.scripts.panel.as_ref().unwrap().command(command, self.scripts.generation),
            Message::Ui(UiEvent::Toggle) => {
                self.scripts.panel = None;
                update_content(self, message)
            }
            Message::Files { .. } | Message::InputClipboard(_) => update_content(self, message),
            _ => Task::none(),
        }
    }

    fn script_event(&mut self, event: Event) -> Task<Message> {
        let panel = self.scripts.panel.as_mut().expect("active script view");
        let generation = self.scripts.generation;
        match (event, &mut panel.stage) {
            (Event::Back, _) => self.script_back(),
            (Event::RunAgain, Stage::Finished(report)) => {
                let report = Arc::clone(report);
                self.repeat_script(report)
            }
            (Event::Input(index, value), Stage::Confirm { prompt, .. }) => {
                prompt.change(index, value);
                Task::none()
            }
            (Event::Focus(index), Stage::Confirm { prompt, .. }) => prompt.focus(index, &panel.scroll, &panel.viewport),
            (Event::Run, Stage::Confirm { prompt, .. }) => {
                let inputs = match prompt.take_arguments() {
                    Ok(inputs) => inputs,
                    Err(index) => return prompt.focus(index, &panel.scroll, &panel.viewport),
                };
                panel.start(inputs, generation)
            }
            (Event::StepStarted(index), Stage::Running { step, .. }) => {
                *step = index;
                Task::none()
            }
            (Event::Finished(result), Stage::Running { .. }) => {
                panel.stage = Stage::Finished(Arc::new(result));
                crate::timing::event("script_finished");
                Task::none()
            }
            (Event::Copy, Stage::Finished(report)) if !report.output.is_empty() => {
                let output = report.output.clone();
                panel.copied = true;
                self.write_clipboard(output)
            }
            _ => Task::none(),
        }
    }
}
