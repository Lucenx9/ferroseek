// SPDX-License-Identifier: GPL-3.0-or-later
//! Inspect and manage shortcuts assigned by the desktop.

use std::time::Duration;

use iced::widget::operation::{self, AbsoluteOffset};
use iced::widget::{button, column, container, responsive, row, scrollable, space, text};
use iced::{Element, Font, Length, Task, Theme};

use super::{
    Launcher, Message, divider, footer_style, hint, palette, primary_hint, status_button_style, surface_style,
};
use crate::activation::Activation;
use crate::commands::Builtin;
use crate::global_shortcuts::{self, Operation, Status};
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{ClickTracker, Command};

const SCROLL: &str = "global-shortcuts-scroll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Configure,
    Stop,
}

#[derive(Debug, Clone)]
pub enum Event {
    Back,
    Act(Action),
    CopyCommand,
    Refresh,
    Poll(u64),
    Completed { request: u64, result: Result<Status, String> },
}

#[derive(Default)]
pub(super) struct State {
    pub(super) panel: Option<Panel>,
    generation: u64,
}

#[derive(Default)]
pub(super) struct Panel {
    command: Option<Builtin>,
    copied: bool,
    status: Option<Status>,
    error: Option<String>,
    pending: Option<Action>,
    request: u64,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Configure => "Configure",
            Self::Stop => "Stop",
        }
    }

    fn progress(self) -> &'static str {
        match self {
            Self::Start => "Starting global shortcuts…",
            Self::Configure => "Opening desktop shortcut settings…",
            Self::Stop => "Stopping global shortcuts…",
        }
    }

    async fn operation(self, activation: Activation) -> Operation {
        match self {
            Self::Start => Operation::Start,
            Self::Configure => Operation::Configure(activation.token(String::new()).await.unwrap_or_default()),
            Self::Stop => Operation::Stop,
        }
    }
}

impl Panel {
    fn primary(&self) -> Option<Action> {
        match self.status {
            Some(Status::Stopped) => Some(Action::Start),
            Some(Status::Ready { version: 2.., .. }) => Some(Action::Configure),
            _ => None,
        }
    }

    fn can_stop(&self) -> bool {
        self.command.is_none() && matches!(self.status, Some(Status::Starting | Status::Ready { .. }))
    }

    fn available(&self, action: Action) -> bool {
        self.pending.is_none() && (self.primary() == Some(action) || (action == Action::Stop && self.can_stop()))
    }

    fn request(&mut self, action: Option<Action>, activation: Activation) -> Task<Event> {
        self.pending = action;
        self.request += 1;
        let request = self.request;
        Task::perform(
            async move {
                let operation = match action {
                    Some(action) => action.operation(activation).await,
                    None => Operation::Read,
                };
                global_shortcuts::perform(operation).await
            },
            move |result| Event::Completed { request, result },
        )
    }

    fn complete(&mut self, result: Result<Status, String>) -> Task<Event> {
        let action = self.pending.take();
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                self.error = Some(error);
                return Task::none();
            }
        };
        let stopped_during_setup =
            action != Some(Action::Stop) && self.status == Some(Status::Starting) && status == Status::Stopped;
        self.error = stopped_during_setup
            .then(|| "Shortcut setup ended. Start again or assign shortcuts in desktop settings.".into());
        self.status = Some(status);
        if stopped_during_setup {
            return Task::none();
        }
        let request = self.request;
        Task::perform(zruntime::unblock(|| std::thread::sleep(Duration::from_secs(1))), move |()| Event::Poll(request))
    }

    fn summary(&self) -> &str {
        if let Some(action) = self.pending {
            return action.progress();
        }
        if let Some(error) = &self.error {
            return error;
        }
        match &self.status {
            None => "Checking global shortcuts…",
            Some(Status::Stopped) => "Start global shortcuts for this session. Your desktop handles key assignment.",
            Some(Status::Starting) => "Waiting for the desktop to finish setting up shortcuts.",
            Some(Status::Ready { version: 2.., .. }) if self.command.is_some() => {
                "Choose Configure to open your desktop shortcut settings."
            }
            Some(Status::Ready { version: 2.., bindings }) if bindings.is_empty() => {
                "No shortcut keys reported. Choose Configure to assign them."
            }
            Some(Status::Ready { version: 2.., .. }) => "Shortcut keys assigned by your desktop.",
            Some(Status::Ready { .. }) => "Global shortcuts are active. Change keys in your desktop settings.",
        }
    }

    fn view(&self) -> Element<'_, Event> {
        let header = super::back_refresh_header(
            if self.command.is_some() { "Global shortcut" } else { "Global shortcuts" },
            Event::Back,
            self.pending.is_none().then_some(Event::Refresh),
        );
        let summary_role = if self.error.is_some() && self.pending.is_none() { palette::ERROR } else { palette::DIM };
        let summary = text(self.summary().to_owned())
            .size(13)
            .style(move |theme: &Theme| text::Style { color: Some(summary_role.resolve(theme)) });
        let body = self.body(summary);
        let content = scrollable(body.padding(12)).id(SCROLL).height(Length::Fill);
        container(column![header, divider(), content, divider(), self.footer()])
            .padding(PAD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(surface_style)
            .into()
    }

    fn body(&self, summary: iced::widget::Text<'static>) -> iced::widget::Column<'static, Event> {
        let bindings = match &self.status {
            Some(Status::Ready { bindings, .. }) => Some(bindings.as_slice()),
            _ => None,
        };
        if let Some(command) = self.command {
            let title = command.title();
            let keys = bindings.map(|bindings| {
                bindings.iter().find(|(name, _)| name == title).map_or("No key reported", |(_, keys)| keys.as_str())
            });
            let manual = column![
                text("Manual desktop shortcut").size(13),
                text(launch_command(command)).size(13).font(Font::MONOSPACE).wrapping(text::Wrapping::WordOrGlyph),
            ]
            .spacing(4);
            return column![binding_row(title, keys), summary, manual].spacing(12);
        }
        let body = column![summary].spacing(12);
        bindings.into_iter().flatten().fold(body, |body, (title, keys)| body.push(binding_row(title, Some(keys))))
    }

    fn footer(&self) -> Element<'_, Event> {
        let controls = responsive(move |size| self.footer_controls(size.width < 320.0)).height(FOOTER_HEIGHT);
        container(controls).height(FOOTER_HEIGHT).padding([0.0, PAD]).align_y(iced::Center).style(footer_style).into()
    }

    fn footer_controls(&self, compact: bool) -> Element<'static, Event> {
        let mut controls = row![].align_y(iced::Center);
        if self.command.is_some() {
            let label = if self.copied { "Launch command copied" } else { "Copy launch command" };
            let copy = if compact { text(label).size(12).into() } else { hint("Ctrl+Shift+C", label) };
            controls =
                controls.push(button(copy).on_press(Event::CopyCommand).padding([4, 8]).style(status_button_style));
        } else if self.can_stop() {
            controls = controls.push(
                button(hint("Ctrl+Enter", "Stop"))
                    .padding([4, 8])
                    .style(status_button_style)
                    .on_press_maybe(self.available(Action::Stop).then_some(Event::Act(Action::Stop))),
            );
        }
        controls = controls.push(space::horizontal());
        if let Some(action) = self.primary() {
            controls = controls.push(
                button(primary_hint("Enter", action.label()))
                    .padding([4, 8])
                    .style(status_button_style)
                    .on_press_maybe(self.available(action).then_some(Event::Act(action))),
            );
        }
        container(controls).center_y(Length::Fill).into()
    }
}

fn binding_row(title: &str, keys: Option<&str>) -> Element<'static, Event> {
    let mut content = row![text(title.to_owned()).size(15).width(Length::FillPortion(3))].spacing(12);
    if let Some(keys) = keys {
        content = content.push(
            text(keys.to_owned())
                .size(13)
                .width(Length::FillPortion(2))
                .align_x(iced::alignment::Horizontal::Right)
                .wrapping(text::Wrapping::WordOrGlyph)
                .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
        );
    }
    content.into()
}

fn launch_command(command: Builtin) -> String {
    format!("ferroseek-launcher --command {}", command.id())
}

impl Launcher {
    pub(super) fn show_global_shortcuts(&mut self, command: Option<Builtin>) -> Task<Message> {
        if self.opening.is_some() {
            return Task::none();
        }
        self.settings = self.settings.map(|_| super::settings::Entry::GlobalShortcuts);
        self.panel = None;
        self.clicks = ClickTracker::default();
        self.global_shortcuts.generation += 1;
        self.global_shortcuts.panel = Some(Panel { command, ..Panel::default() });
        let load = self.global_shortcut_event(Event::Refresh);
        Task::batch([load, self.resize()])
    }

    pub(super) fn global_shortcut_view<'a>(&self, panel: &'a Panel) -> Element<'a, Message> {
        let generation = self.global_shortcuts.generation;
        panel.view().map(move |event| Message::GlobalShortcuts { generation, event })
    }

    pub(super) fn update_global_shortcuts(&mut self, message: Message) -> Task<Message> {
        let event = match message {
            Message::GlobalShortcuts { generation, event } if generation == self.global_shortcuts.generation => event,
            Message::Cmd(Command::Back) => Event::Back,
            Message::Cmd(Command::Activate) => {
                let Some(action) = self.global_shortcuts.panel.as_ref().and_then(Panel::primary) else {
                    return Task::none();
                };
                Event::Act(action)
            }
            Message::Cmd(Command::Secondary) => Event::Act(Action::Stop),
            Message::Cmd(Command::CopyPath) => Event::CopyCommand,
            Message::RefreshSearchStatus => Event::Refresh,
            Message::Cmd(command) => return scroll(command),
            Message::ShowSettings | Message::ShowAppearance | Message::ShowFolders | Message::ShowSearchStatus => {
                return Task::none();
            }
            message => return super::update_content(self, message),
        };
        self.global_shortcut_event(event)
    }

    fn global_shortcut_event(&mut self, event: Event) -> Task<Message> {
        let Some(panel) = &mut self.global_shortcuts.panel else { return Task::none() };
        let generation = self.global_shortcuts.generation;
        let task = match event {
            Event::Back => {
                self.global_shortcuts.panel = None;
                return self.return_to_parent();
            }
            Event::CopyCommand => {
                let Some(command) = panel.command else { return Task::none() };
                panel.copied = true;
                return self.write_clipboard(launch_command(command));
            }
            Event::Act(action) if panel.available(action) => panel.request(Some(action), self.activation.clone()),
            Event::Refresh if panel.pending.is_none() => panel.request(None, self.activation.clone()),
            Event::Poll(request) if request == panel.request => panel.request(None, self.activation.clone()),
            Event::Completed { request, result } if request == panel.request => panel.complete(result),
            _ => Task::none(),
        };
        task.map(move |event| Message::GlobalShortcuts { generation, event })
    }
}

fn scroll(command: Command) -> Task<Message> {
    let y = match command {
        Command::Up => -48.0,
        Command::Down => 48.0,
        Command::PageUp => -240.0,
        Command::PageDown => 240.0,
        _ => return Task::none(),
    };
    operation::scroll_by(SCROLL, AbsoluteOffset { x: 0.0, y })
}
