// SPDX-License-Identifier: GPL-3.0-or-later
//! Current metadata for one captured file; never opens the index or file contents.
use iced::alignment::Vertical;
use iced::widget::{Id, column, container, responsive, row, scrollable, text};
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
        let refresh = (!matches!(self.state, State::Loading)).then_some(Event::Refresh);
        let (location, url) = self.location();
        let copy = if url { "Copy file URL" } else { "Copy path" };
        let mut body = column![
            text(&self.file.name).size(18).wrapping(text::Wrapping::WordOrGlyph),
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
            State::Loading => {}
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
        container(column![
            ui::back_refresh_header("File details", Event::Back, refresh),
            ui::divider(),
            scrollable(container(body).padding([12, 16])).id(scroll_id()).height(Length::Fill),
            ui::divider(),
            self.footer(copy),
        ])
        .padding(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(ui::surface_style)
        .into()
    }

    fn footer(&self, copy_label: &'static str) -> Element<'_, Event> {
        let (status, color) = match (self.notice, &self.state) {
            (Some(notice), _) => (notice, palette::ACCENT),
            (_, State::Loading) => ("Loading details…", palette::DIM),
            (_, State::Error(_)) => ("Details unavailable", palette::ERROR),
            (_, State::Ready(_)) => ("", palette::DIM),
        };
        let content = responsive(move |size| {
            let mut controls = row![
                text(status)
                    .size(12)
                    .width(Length::Fill)
                    .wrapping(text::Wrapping::None)
                    .style(move |theme: &iced::Theme| text::Style { color: Some(color.resolve(theme)) })
            ]
            .spacing(8)
            .align_y(Vertical::Center);
            let (copy, preview) = if size.width >= 560.0 {
                controls = controls.push(ui::footer_button(ui::hint("Esc", "Back to search"), Some(Event::Back)));
                (ui::hint("Ctrl+Shift+C", copy_label), ui::primary_hint("Enter", "Preview text"))
            } else {
                (text(copy_label).size(12).into(), ui::primary_label("Preview text"))
            };
            let controls = controls
                .push(ui::footer_button(copy, Some(Event::CopyLocation)))
                .push(ui::footer_button(preview, Some(Event::Preview)));
            container(controls).center_y(Length::Fill).into()
        });
        container(content)
            .padding(iced::padding::left(16).right(8))
            .height(crate::layout::FOOTER_HEIGHT)
            .center_y(crate::layout::FOOTER_HEIGHT)
            .style(ui::footer_style)
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
