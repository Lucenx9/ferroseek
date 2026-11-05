// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, read-only text preview of one captured indexed file.
use iced::alignment::Vertical;
use iced::widget::{Id, button, column, container, row, scrollable, space, text};
use iced::{Element, Font, Length};

use crate::backend::{FileHit, PreviewData, TextPreview};
use crate::model::{self, Command};
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
    details: Option<Result<crate::backend::FileDetails, String>>,
}

pub fn scroll_id() -> Id {
    Id::new("text-preview-scroll")
}

impl Panel {
    pub fn new(file: FileHit) -> Self {
        Self { file, state: State::Loading, notice: None, details: None }
    }

    pub fn loaded(&mut self, result: Result<PreviewData, String>) {
        let content = match result {
            Ok(data) => {
                self.details = Some(data.details);
                data.content
            }
            Err(error) => {
                self.details = Some(Err(error.clone()));
                Err(error)
            }
        };
        self.state = match content {
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
                self.details = None;
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
            Command::ScrollPreview(direction) => Effect::Scroll(f32::from(direction) * 240.0),
            _ => Effect::None,
        }
    }

    pub fn view(&self) -> Element<'_, Event> {
        let mut refresh = button(ui::hint("Ctrl+R", "Refresh")).style(ui::status_button_style);
        if !matches!(self.state, State::Loading) {
            refresh = refresh.on_press(Event::Refresh);
        }
        let header = row![
            text("Preview").size(13).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::DIM.resolve(theme)),
            }),
            space::horizontal(),
            refresh,
            button(ui::hint("Esc", "Hide")).on_press(Event::Back).style(ui::status_button_style)
        ]
        .spacing(6)
        .align_y(Vertical::Center);
        let (location, url) = self.location();
        let display = if url { location.to_owned() } else { model::home_relative(location) };
        let copy = if url { "Copy file URL" } else { "Copy path" };
        let identity = column![
            text(ui::ellipsize_middle(&self.file.name, 65)).size(15),
            iced::widget::tooltip(
                text(ui::ellipsize_middle(&display, 75)).size(11).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }),
                container(text(location).size(12).wrapping(text::Wrapping::WordOrGlyph))
                    .padding(8)
                    .max_width(560)
                    .style(ui::surface_style),
                iced::widget::tooltip::Position::Bottom
            ),
        ]
        .spacing(4);
        let mut body = column![].spacing(12).width(Length::Fill);
        let mut truncated = false;
        match &self.state {
            State::Loading => {
                body = body.push(text("Loading text preview…").size(15).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }))
            }
            State::Error(error) => {
                body = body.push(text("Text preview could not be loaded").size(16));
                body = body.push(text(ui::ellipsize_end(error, 300)).size(13).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::ERROR.resolve(theme)) }
                }));
                body = body.push(text("Refresh to try again, or select another file.").size(12).style(
                    move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) },
                ));
            }
            State::Ready(TextPreview::Unsupported(reason)) => {
                body = body.push(text("No text preview available").size(16));
                body = body.push(text(ui::ellipsize_end(reason, 300)).size(13).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }));
                body = body.push(text("Enter opens the file in its application.").size(12).style(
                    move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) },
                ));
            }
            State::Ready(TextPreview::Text { text: contents, truncated: clipped }) => {
                truncated = *clipped;
                if contents.is_empty() {
                    body = body.push(text("Empty text file").size(15).style(move |theme: &iced::Theme| {
                        iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                    }));
                } else {
                    body =
                        body.push(text(contents).font(Font::MONOSPACE).size(13).wrapping(text::Wrapping::WordOrGlyph));
                }
            }
        }
        let mut notes = row![].spacing(12).align_y(Vertical::Center);
        if let Some(notice) = self.notice {
            notes = notes.push(text(notice).size(11).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(palette::ACCENT.resolve(theme)),
            }));
        } else if truncated {
            notes = notes.push(text("Beginning of file").size(11).style(move |theme: &iced::Theme| {
                iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
            }));
        }
        let mut metadata = column![
            row![
                text("File details").size(12).style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(palette::DIM.resolve(theme)),
                }),
                space::horizontal(),
                button(text(copy).size(11)).on_press(Event::CopyLocation).style(ui::status_button_style)
            ]
            .align_y(Vertical::Center)
        ]
        .spacing(5);
        match &self.details {
            None => {
                metadata = metadata.push(text("Loading details…").size(12).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }))
            }
            Some(Err(_)) => {
                metadata = metadata.push(text("File details unavailable").size(12).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }))
            }
            Some(Ok(details)) => {
                let mime = if details.mime.is_empty() { "Unknown file type" } else { &details.mime };
                let modified = chrono::DateTime::from_timestamp(details.modified, 0)
                    .map(|date| date.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| "Unavailable".into());
                metadata = metadata
                    .push(metadata_row("Type", mime.to_owned()))
                    .push(metadata_row("Size", crate::details::format_size(details.size)))
                    .push(metadata_row("Modified", modified));
            }
        }
        column![
            header,
            identity,
            scrollable(container(body).padding(iced::padding::top(8).bottom(8).right(12)))
                .id(scroll_id())
                .height(Length::Fill),
            notes,
            ui::divider(),
            metadata,
            container(ui::hint("Alt+PgUp/PgDn", "Scroll")).width(Length::Fill).clip(true)
        ]
        .spacing(8)
        .padding([8, 16])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn metadata_row(label: &str, value: String) -> Element<'static, Event> {
    row![
        text(label.to_owned())
            .size(11)
            .style(move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) })
            .width(64),
        text(value).size(12).wrapping(text::Wrapping::WordOrGlyph)
    ]
    .spacing(8)
    .into()
}

pub fn empty_view() -> Element<'static, Event> {
    container(
        column![
            row![
                text("Preview").size(13).style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(palette::DIM.resolve(theme)),
                }),
                space::horizontal(),
                button(ui::hint("Esc", "Hide")).on_press(Event::Back).style(ui::status_button_style)
            ],
            space::vertical(),
            text("Select a file to preview").size(16),
            text("Applications and folders have no text preview.").size(12).style(move |theme: &iced::Theme| {
                iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
            }),
            space::vertical(),
        ]
        .spacing(12),
    )
    .padding([8, 16])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
