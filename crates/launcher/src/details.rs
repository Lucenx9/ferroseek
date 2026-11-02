// SPDX-License-Identifier: GPL-3.0-or-later
//! Current metadata for one captured file; never opens the index or file contents.
use iced::alignment::Vertical;
use iced::widget::{Id, button, column, container, row, scrollable, space, text};
use iced::{Element, Length};

use crate::backend::{FileDetails, FileHit};
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
    Ready(FileDetails),
    Error(String),
}

pub struct Panel {
    pub file: FileHit,
    state: State,
    notice: Option<&'static str>,
}

pub fn scroll_id() -> Id {
    Id::new("file-details-scroll")
}

impl Panel {
    pub fn new(file: FileHit) -> Self {
        Self { file, state: State::Loading, notice: None }
    }

    pub fn loaded(&mut self, result: Result<FileDetails, String>) {
        self.state = match result {
            Ok(details) => State::Ready(details),
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
                let location = location.to_string();
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

    pub fn view(&self) -> Element<'_, Event> {
        let mut refresh = button(ui::hint("Ctrl+R", "Refresh")).style(ui::status_button_style);
        if !matches!(self.state, State::Loading) {
            refresh = refresh.on_press(Event::Refresh);
        }
        let header = row![text("File details").size(20), space::horizontal(), refresh].align_y(Vertical::Center);
        let (location, url) = self.location();
        let copy = if url { "Copy file URL" } else { "Copy path" };
        let mut body = column![
            field("Name", &self.file.name),
            column![
                row![
                    text(if url { "File URL" } else { "Location" }).size(12).color(palette::DIM),
                    space::horizontal(),
                    button(text(copy).size(12)).on_press(Event::CopyLocation).style(ui::status_button_style)
                ]
                .align_y(Vertical::Center),
                text(location).size(15).wrapping(text::Wrapping::WordOrGlyph)
            ]
            .spacing(4),
        ]
        .spacing(16);
        match &self.state {
            State::Loading => body = body.push(text("Loading file details…").size(15).color(palette::DIM)),
            State::Error(error) => {
                body = body.push(text("File details could not be loaded").size(16));
                body = body.push(text(ui::ellipsize_end(error, 300)).size(13).color(palette::ERROR));
                body =
                    body.push(text("Refresh to try again, or Esc to return to search.").size(12).color(palette::DIM));
            }
            State::Ready(details) => {
                let mime = if details.mime.is_empty() { "Unknown file type" } else { &details.mime };
                body = body.push(field("Type", mime));
                body = body.push(field("Size", &format_size(details.size)));
                let modified = chrono::DateTime::from_timestamp(details.modified, 0)
                    .map(|date| date.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| "Unavailable".into());
                body = body.push(field("Modified", &modified));
            }
        }
        let mut footer = row![ui::hint("Esc", "Back to search"), space::horizontal()].align_y(Vertical::Center);
        if let Some(notice) = self.notice {
            footer = footer.push(text(notice).size(12).color(palette::ACCENT)).push(space::horizontal());
        }
        footer = footer.push(ui::hint("Ctrl+Shift+C", copy));
        container(
            column![
                container(header).padding([8, 16]),
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

fn field(label: &str, value: &str) -> Element<'static, Event> {
    column![
        text(label.to_owned()).size(12).color(palette::DIM),
        text(value.to_owned()).size(15).wrapping(text::Wrapping::WordOrGlyph)
    ]
    .spacing(4)
    .into()
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "bytes";
    for next in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        value /= 1024.0;
        unit = next;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit} ({bytes} bytes)")
}
