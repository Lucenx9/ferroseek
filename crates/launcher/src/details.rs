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
    Preview,
}

pub enum Effect {
    None,
    Close,
    Refresh,
    Copy(String),
    Preview,
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
            Event::Preview => Effect::Preview,
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
            Command::Activate => self.event(Event::Preview),
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
        let mut refresh = button(ui::hint("Ctrl+R", "Refresh")).padding([6, 8]).style(ui::status_button_style);
        if !matches!(self.state, State::Loading) {
            refresh = refresh.on_press(Event::Refresh);
        }
        let header = row![
            text("File details").size(20),
            space::horizontal(),
            button(ui::hint("Enter", "Preview text")).on_press(Event::Preview).style(ui::status_button_style),
            refresh
        ]
        .align_y(Vertical::Center);
        let (location, url) = self.location();
        let copy = if url { "Copy file URL" } else { "Copy path" };
        let mut body = column![
            field("Name", &self.file.name),
            column![
                text(if url { "File URL" } else { "Location" }).size(12).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }),
                text(location).size(15).wrapping(text::Wrapping::WordOrGlyph)
            ]
            .spacing(4),
        ]
        .spacing(16);
        match &self.state {
            State::Loading => {
                body = body.push(text("Loading file details…").size(15).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }))
            }
            State::Error(error) => {
                body = body.push(text("File details could not be loaded").size(16));
                body = body.push(text(ui::ellipsize_end(error, 300)).size(13).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::ERROR.resolve(theme)) }
                }));
                body = body.push(text("Refresh to try again, or Esc to return to search.").size(12).style(
                    move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) },
                ));
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
        let mut footer = row![
            button(ui::hint("Esc", "Back to search"))
                .on_press(Event::Back)
                .padding([4, 0])
                .style(ui::status_button_style),
            space::horizontal()
        ]
        .align_y(Vertical::Center);
        if let Some(notice) = self.notice {
            footer = footer
                .push(text(notice).size(12).style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(palette::ACCENT.resolve(theme)),
                }))
                .push(space::horizontal());
        }
        footer = footer.push(
            button(ui::hint("Ctrl+Shift+C", copy))
                .on_press(Event::CopyLocation)
                .padding([4, 8])
                .style(ui::status_button_style),
        );
        container(column![
            ui::page_header(header),
            ui::divider(),
            scrollable(container(body).padding([12, 16])).id(scroll_id()).height(Length::Fill),
            ui::divider(),
            container(footer).padding(iced::padding::left(16).right(8)).height(34).center_y(34).style(ui::footer_style),
        ])
        .padding(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(ui::surface_style)
        .into()
    }
}

fn field(label: &str, value: &str) -> Element<'static, Event> {
    column![
        text(label.to_owned())
            .size(12)
            .style(move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }),
        text(value.to_owned()).size(15).wrapping(text::Wrapping::WordOrGlyph)
    ]
    .spacing(4)
    .into()
}

pub(crate) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "bytes";
    for next in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        value /= 1024.0;
        unit = next;
        // Select the unit from the rounded value users will see.
        if (value * 10.0).round() / 10.0 < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit} ({bytes} bytes)")
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn displayed_sizes_use_the_next_unit_when_rounding_crosses_its_boundary() {
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(1024), "1.0 KiB (1024 bytes)");
        assert_eq!(format_size(1_048_000), "1023.4 KiB (1048000 bytes)");
        assert_eq!(format_size(1_048_575), "1.0 MiB (1048575 bytes)");
        assert_eq!(format_size(1_073_741_823), "1.0 GiB (1073741823 bytes)");
    }
}
