// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, read-only text preview of one captured indexed file.
use iced::alignment::Vertical;
use iced::widget::{Id, button, column, container, row, scrollable, space, text};
use iced::{Element, Font, Length};

use crate::backend::{FileHit, TextPreview};
use crate::model::Command;
use crate::ui::{self, palette};

#[derive(Debug, Clone)]
pub enum Event {
    Back,
    Refresh,
    CopyLocation,
}

pub enum Effect {
    None,
    Close,
    Refresh,
    Copy(String),
    Scroll(f32),
    Edge(bool),
}

enum State {
    Loading,
    Ready(TextPreview),
    Error(String),
}

pub struct Panel {
    pub file: FileHit,
    state: State,
    notice: Option<&'static str>,
}

pub fn scroll_id() -> Id {
    Id::new("text-preview-scroll")
}

impl Panel {
    pub fn new(file: FileHit) -> Self {
        Self { file, state: State::Loading, notice: None }
    }

    pub fn loaded(&mut self, result: Result<TextPreview, String>) {
        self.state = match result {
            Ok(preview) => State::Ready(preview),
            Err(error) => State::Error(error),
        };
    }

    fn location(&self) -> (&str, bool) {
        match self.file.path.as_deref().and_then(|path| path.to_str()) {
            Some(path) => (path, false),
            None => (&self.file.id, true),
        }
    }

    pub fn event(&mut self, event: Event) -> Effect {
        match event {
            Event::Back => Effect::Close,
            Event::Refresh if !matches!(self.state, State::Loading) => {
                self.state = State::Loading;
                self.notice = None;
                Effect::Refresh
            }
            Event::CopyLocation => {
                let (location, url) = self.location();
                let location = location.to_owned();
                self.notice = Some(if url { "Copied file URL" } else { "Copied path" });
                Effect::Copy(location)
            }
            _ => Effect::None,
        }
    }

    pub fn command(&mut self, command: Command) -> Effect {
        match command {
            Command::Back => self.event(Event::Back),
            Command::CopyPath => self.event(Event::CopyLocation),
            Command::Up => Effect::Scroll(-48.0),
            Command::Down => Effect::Scroll(48.0),
            Command::PageUp => Effect::Scroll(-240.0),
            Command::PageDown => Effect::Scroll(240.0),
            Command::First => Effect::Edge(false),
            Command::Last => Effect::Edge(true),
            _ => Effect::None,
        }
    }

    pub fn view(&self, from_details: bool) -> Element<'_, Event> {
        let mut refresh = button(ui::hint("Ctrl+R", "Refresh")).style(ui::status_button_style);
        if !matches!(self.state, State::Loading) {
            refresh = refresh.on_press(Event::Refresh);
        }
        let header = row![text("Text preview").size(20), space::horizontal(), refresh].align_y(Vertical::Center);
        let (location, url) = self.location();
        let copy = if url { "Copy file URL" } else { "Copy path" };
        let identity = column![
            text(ui::ellipsize_end(&self.file.name, 80)).size(16),
            text(ui::ellipsize_end(location, 160)).size(12).color(palette::DIM),
        ]
        .spacing(4);
        let mut body = column![].spacing(12).width(Length::Fill);
        let mut truncated = false;
        match &self.state {
            State::Loading => body = body.push(text("Loading text preview…").size(15).color(palette::DIM)),
            State::Error(error) => {
                body = body.push(text("Text preview could not be loaded").size(16));
                body = body.push(text(ui::ellipsize_end(error, 300)).size(13).color(palette::ERROR));
                body = body.push(text("Refresh to try again, or Esc to go back.").size(12).color(palette::DIM));
            }
            State::Ready(TextPreview::Unsupported(reason)) => {
                body = body.push(text("No text preview available").size(16));
                body = body.push(text(ui::ellipsize_end(reason, 300)).size(13).color(palette::DIM));
                body = body.push(text("Go back to open this file in an application.").size(12).color(palette::DIM));
            }
            State::Ready(TextPreview::Text { text: contents, truncated: clipped }) => {
                truncated = *clipped;
                if contents.is_empty() {
                    body = body.push(text("Empty text file").size(15).color(palette::DIM));
                } else {
                    body =
                        body.push(text(contents).font(Font::MONOSPACE).size(13).wrapping(text::Wrapping::WordOrGlyph));
                }
            }
        }
        let mut footer =
            row![ui::hint("Esc", if from_details { "Back to details" } else { "Back to search" }), space::horizontal()]
                .align_y(Vertical::Center);
        if let Some(notice) = self.notice {
            footer = footer.push(text(notice).size(12).color(palette::ACCENT)).push(space::horizontal());
        } else if truncated {
            footer = footer.push(text("Beginning of file").size(12).color(palette::DIM)).push(space::horizontal());
        }
        footer = footer
            .push(button(ui::hint("Ctrl+Shift+C", copy)).on_press(Event::CopyLocation).style(ui::status_button_style));
        container(
            column![
                container(header).padding([8, 16]),
                container(identity).padding([4, 16]),
                scrollable(container(body).padding([12, 16])).id(scroll_id()).height(Length::Fill),
                container(footer).padding([0, 10]).height(34).center_y(34),
            ]
            .spacing(3),
        )
        .padding(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(ui::surface_style)
        .into()
    }
}
