// SPDX-License-Identifier: GPL-3.0-or-later
//! The shared clipboard publication and focus handoff for explicit Paste actions.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, text};
use iced::{Element, Task, Theme};
use iced_layershell::reexport::KeyboardInteractivity;

use super::{Launcher, Message, Status, label_for_width, palette};
use crate::clipboard_history::publish::{self, Lease};
use crate::window_switcher::PasteTarget;

#[derive(Clone, Copy)]
pub(super) enum Origin {
    History,
    Snippet,
}

enum Phase {
    Publishing,
    Releasing(Arc<Lease>),
    Sending,
}

struct Paste {
    origin: Origin,
    phase: Phase,
}

pub(super) struct State {
    target: Option<PasteTarget>,
    attempt: u64,
    pending: Option<Paste>,
}

impl State {
    pub(super) fn new(target: Option<PasteTarget>) -> Self {
        Self { target, attempt: 0, pending: None }
    }

    pub(super) fn label(&self) -> Option<&str> {
        self.target.as_ref().map(PasteTarget::label)
    }

    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn phase(&self) -> Option<&Phase> {
        self.pending.as_ref().map(|paste| &paste.phase)
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Published(Result<Arc<Lease>, String>),
    RestoreFocus,
    FocusRestored(Result<(), &'static str>),
    FocusTimeout,
    Sent(Result<(), &'static str>),
}

pub(super) fn action_title(label: &str, destination: Option<&str>, width: f32) -> Element<'static, Message> {
    let title = text(label_for_width(label, width, 14.0, false)).size(14).wrapping(text::Wrapping::None);
    let Some(destination) = destination else { return title.into() };
    let destination = text(label_for_width(destination, width, 11.0, false))
        .size(11)
        .wrapping(text::Wrapping::None)
        .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) });
    column![title, destination].into()
}

impl Launcher {
    pub(super) fn paste_text(&mut self, text: String, origin: Origin) -> Task<Message> {
        if self.paste.target.is_none() {
            return Task::none();
        }
        if self.paste.busy() || self.opening.is_some() {
            return Task::none();
        }
        self.paste.attempt = self.paste.attempt.wrapping_add(1);
        self.paste.pending = Some(Paste { origin, phase: Phase::Publishing });
        self.show_paste_progress(origin);
        let attempt = self.paste.attempt;
        let publish = zruntime::unblock(move || publish::handoff(&text).map(Arc::new).map_err(|_| text));
        Task::perform(publish, move |result| Message::Paste { attempt, event: Event::Published(result) })
    }

    pub(super) fn route_paste(&mut self, message: &Message) -> Option<Task<Message>> {
        match message {
            Message::Paste { attempt, event } => Some(self.update_paste(*attempt, event.clone())),
            Message::PasteSnippet if self.search_suspended() => Some(Task::none()),
            Message::PasteSnippet => Some(self.perform(super::Action::PasteSnippet)),
            _ if !self.paste.busy() => None,
            Message::FocusLost => Some(self.paste_focus_lost()),
            Message::Cmd(_)
            | Message::Query(_)
            | Message::ActionQuery(_)
            | Message::Selected(_)
            | Message::Clicked(_)
            | Message::ContextMenu(_)
            | Message::PanelClicked { .. }
            | Message::Scope(_)
            | Message::ContextQuery { .. }
            | Message::Wheel(_)
            | Message::ResultsWheel(_)
            | Message::ShowAppearance
            | Message::ShowFolders
            | Message::ShowSettings
            | Message::ShowSearchStatus
            | Message::FolderShortcut(_)
            | Message::ClipboardShortcut(_)
            | Message::InputClipboard(_) => Some(Task::none()),
            _ => None,
        }
    }

    fn show_paste_progress(&mut self, origin: Origin) {
        match origin {
            Origin::History => self.clipboard_history.panel.as_mut().unwrap().paste_feedback(None),
            Origin::Snippet => {
                self.panel = None;
                self.status = Status::Notice("Pasting snippet…".into());
            }
        }
    }

    fn update_paste(&mut self, attempt: u64, event: Event) -> Task<Message> {
        if self.paste.attempt != attempt || !self.paste.busy() {
            return Task::none();
        }
        match (event, self.paste.phase()) {
            (Event::Published(Ok(lease)), Some(Phase::Publishing)) => self.release_for_paste(lease),
            (Event::Published(Err(text)), Some(Phase::Publishing)) => {
                let restore = self.finish_paste_error("Copied text. Keep FerroSeek open and paste manually.");
                Task::batch([self.write_clipboard(text), restore])
            }
            (Event::RestoreFocus, Some(Phase::Releasing(lease))) => self.restore_paste_focus(Arc::clone(lease)),
            (Event::FocusRestored(Err(error)), Some(Phase::Releasing(_))) => self.finish_paste_error(error),
            (Event::FocusTimeout, Some(Phase::Releasing(_))) => {
                self.finish_paste_error("Could not confirm the focus change. Paste the copied text manually.")
            }
            (Event::Sent(Ok(())), Some(Phase::Sending)) => {
                crate::timing::event("clipboard_paste_submitted");
                self.paste.pending = None;
                iced::exit()
            }
            (Event::Sent(Err(error)), Some(Phase::Sending)) => self.finish_paste_error(error),
            _ => Task::none(),
        }
    }

    fn release_for_paste(&mut self, lease: Arc<Lease>) -> Task<Message> {
        crate::timing::event("clipboard_handoff_ready");
        self.clipboard_owner = false;
        self.paste.pending.as_mut().expect("publishing a paste").phase = Phase::Releasing(lease);
        let attempt = self.paste.attempt;
        let timeout = async {
            if let Ok(runtime) = zruntime::SharedRuntime::current() {
                runtime.sleep(Duration::from_millis(750)).await;
            }
        };
        Task::batch([
            Task::done(Message::KeyboardInteractivityChange(KeyboardInteractivity::None))
                .chain(Task::done(Message::Paste { attempt, event: Event::RestoreFocus })),
            Task::perform(timeout, move |()| Message::Paste { attempt, event: Event::FocusTimeout }),
        ])
    }

    fn restore_paste_focus(&mut self, lease: Arc<Lease>) -> Task<Message> {
        let Some(target) = &self.paste.target else { return Task::none() };
        let attempt = self.paste.attempt;
        Task::perform(target.restore_focus(lease), move |result| Message::Paste {
            attempt,
            event: Event::FocusRestored(result),
        })
    }

    fn paste_focus_lost(&mut self) -> Task<Message> {
        let Some(Phase::Releasing(lease)) = self.paste.phase() else { return Task::none() };
        let Some(target) = &self.paste.target else {
            return self.finish_paste_error("The original window is unavailable. Paste the copied text manually.");
        };
        let send = target.send(Arc::clone(lease));
        self.paste.pending.as_mut().expect("releasing focus for a paste").phase = Phase::Sending;
        let attempt = self.paste.attempt;
        Task::perform(send, move |result| Message::Paste { attempt, event: Event::Sent(result) })
    }

    fn finish_paste_error(&mut self, error: &'static str) -> Task<Message> {
        let Some(paste) = self.paste.pending.take() else { return Task::none() };
        crate::timing::event("clipboard_paste_rejected");
        match paste.origin {
            Origin::History => self.clipboard_history.panel.as_mut().unwrap().paste_feedback(Some(error)),
            Origin::Snippet => self.status = Status::Error(error.into()),
        }
        let focus = self.input_focus().map_or_else(Task::none, crate::input::focus);
        if matches!(paste.phase, Phase::Publishing) {
            return focus;
        }
        Task::batch([Task::done(Message::KeyboardInteractivityChange(KeyboardInteractivity::Exclusive)), focus])
    }
}
