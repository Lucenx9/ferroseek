// SPDX-License-Identifier: GPL-3.0-or-later
//! Confirmation, output and return to search for local commands.

use std::path::Path;
use std::sync::Arc;

use iced::widget::operation::{self, AbsoluteOffset, RelativeOffset};
use iced::widget::{Id, column, container, row, scrollable, space, text};
use iced::{Element, Font, Length, Task, Theme};

use super::script_prompt::Prompt;
use super::{Launcher, Message, Status, divider, footer_button, footer_style, hint, label_for_width, page_header};
use super::{palette, primary_hint, surface_style, update_content};
use crate::backend::UiEvent;
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{Action, Command, Scope, Target};
use crate::script_commands::{Catalog, Completion, Job, Report, Script};

#[derive(Default)]
pub(super) struct State {
    pub catalog: Catalog,
    pub panel: Option<Panel>,
    pub editor: Option<super::script_editor::Editor>,
    pub generation: u64,
}

impl State {
    pub fn is_open(&self) -> bool {
        self.panel.is_some() || self.editor.is_some()
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
    Copy,
    Finished(Result<Report, String>),
    Input(usize, String),
    Focus(usize),
}

enum Stage {
    Confirm(Prompt),
    Running { _job: Job },
    Finished(Result<Report, String>),
}

pub(super) struct Panel {
    script: Arc<Script>,
    stage: Stage,
    scroll: Id,
    viewport: Id,
    copied: bool,
}

impl Panel {
    fn new(script: Arc<Script>) -> Self {
        let prompt = Prompt::new(script.parameters.len());
        Self { script, stage: Stage::Confirm(prompt), scroll: Id::unique(), viewport: Id::unique(), copied: false }
    }

    pub(super) fn prompt(&self) -> Option<&Prompt> {
        match &self.stage {
            Stage::Confirm(prompt) => Some(prompt),
            _ => None,
        }
    }

    fn primary(&self) -> Option<(&'static str, &'static str, Event)> {
        match &self.stage {
            Stage::Confirm(_) => Some(("Ctrl+Enter", "Run", Event::Run)),
            Stage::Finished(Ok(report)) if !report.output.is_empty() => {
                let label = if self.copied { "Copied output" } else { "Copy output" };
                Some(("Enter", label, Event::Copy))
            }
            _ => None,
        }
    }

    pub(super) fn view(&self, generation: u64) -> Element<'_, Message> {
        let title = self.script.title.as_str();
        let header = page_header(
            iced::widget::responsive(move |size| {
                text(label_for_width(title, size.width, 20.0, false)).size(20).wrapping(text::Wrapping::None).into()
            })
            .height(Length::Shrink),
        )
        .height(56);
        let scroll = container(scrollable(self.body(generation)).id(self.scroll.clone()).height(Length::Fill))
            .id(self.viewport.clone());
        let back = if matches!(self.stage, Stage::Running { .. }) { "Cancel" } else { "Back" };
        let event = |event| Message::Script { generation, event };
        let mut actions = row![footer_button(hint("Esc", back), event(Event::Back)), space::horizontal()]
            .align_y(iced::Center)
            .spacing(6);
        if let Some((key, label, action)) = self.primary() {
            actions = actions.push(footer_button(primary_hint(key, label), event(action)));
        }
        let footer =
            container(actions).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style);
        container(column![header, divider(), scroll, divider(), footer])
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into()
    }

    fn body(&self, generation: u64) -> Element<'_, Message> {
        let body = match &self.stage {
            Stage::Confirm(prompt) => self.confirmation(prompt, generation),
            Stage::Running { .. } => column![text("Running…").size(15)],
            Stage::Finished(Err(error)) => column![error_text(error)],
            Stage::Finished(Ok(report)) => report_body(report),
        };
        body.spacing(8).padding(16).into()
    }

    fn confirmation<'a>(&'a self, prompt: &'a Prompt, generation: u64) -> iced::widget::Column<'a, Message> {
        let mut body = column![];
        if self.script.confirm {
            let executable = self.script.executable.display();
            let args = self.script.args.iter().map(|arg| format!("{arg:?}")).collect::<Vec<_>>().join(" ");
            body = body.push(text("Run this command?").size(15)).push(output_text(format!("{executable}\n{args}")));
        }
        body.push(prompt.view(&self.script.parameters, generation))
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

fn output_text<'a>(value: impl iced::widget::text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(value).font(Font::MONOSPACE).size(13).wrapping(text::Wrapping::WordOrGlyph)
}

fn error_text(value: &str) -> iced::widget::Text<'_> {
    text(value).size(15).style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) })
}

fn report_body(report: &Report) -> iced::widget::Column<'_, Message> {
    let summary = match report.completion {
        Completion::Exited(status) if status.success() => text("Completed").size(15),
        Completion::Exited(status) => {
            let message = format!("Command failed ({status})");
            text(message).size(15).style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) })
        }
        Completion::Cancelled => text("Cancelled").size(15),
        Completion::TimedOut => error_text("Stopped after 30 seconds."),
    };
    let mut body = column![summary];
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
    pub(super) fn activate_command(&mut self, target: &Target, action: Action) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        match (target, action) {
            (Target::Command(command), Action::OpenCommand) => self.open_builtin(*command),
            (Target::Script(script), Action::RunScript) => self.open_script(script.clone()),
            (Target::Script(script), Action::EditScript) => self.open_script_editor(Some(script.clone())),
            _ => Task::none(),
        }
    }

    fn open_script(&mut self, script: Arc<Script>) -> Task<Message> {
        let manual = script.confirm || !script.parameters.is_empty();
        self.panel = None;
        self.scripts.generation += 1;
        self.scripts.panel = Some(Panel::new(script));
        let action = if manual { self.script_event(Event::Focus(0)) } else { self.script_event(Event::Run) };
        Task::batch([self.resize(), action])
    }

    pub(super) fn update_scripts(&mut self, message: Message) -> Task<Message> {
        if self.scripts.editor.is_some() {
            return self.update_script_editor(message);
        }
        match message {
            Message::Script { generation, event } if generation == self.scripts.generation => self.script_event(event),
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
            (Event::Back, _) => {
                self.scripts.panel = None;
                self.return_to_parent()
            }
            (Event::Input(index, value), Stage::Confirm(prompt)) => {
                prompt.change(index, value);
                Task::none()
            }
            (Event::Focus(index), Stage::Confirm(prompt)) => prompt.focus(index, &panel.scroll, &panel.viewport),
            (Event::Run, Stage::Confirm(prompt)) => {
                let inputs = match prompt.take_arguments() {
                    Ok(inputs) => inputs,
                    Err(index) => return prompt.focus(index, &panel.scroll, &panel.viewport),
                };
                let (job, work) = Job::start(panel.script.clone(), inputs);
                panel.stage = Stage::Running { _job: job };
                crate::timing::event("script_started");
                Task::perform(work, move |result| Message::Script { generation, event: Event::Finished(result) })
            }
            (Event::Finished(result), Stage::Running { .. }) => {
                panel.stage = Stage::Finished(result);
                crate::timing::event("script_finished");
                Task::none()
            }
            (Event::Copy, Stage::Finished(Ok(report))) if !report.output.is_empty() => {
                let output = report.output.clone();
                panel.copied = true;
                self.write_clipboard(output)
            }
            _ => Task::none(),
        }
    }
}
