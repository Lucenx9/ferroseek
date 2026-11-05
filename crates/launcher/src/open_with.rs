// SPDX-License-Identifier: GPL-3.0-or-later
//! A secondary application chooser bound to one immutable file result.
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{self, Id, button, column, container, image, row, space, svg, text, text_input};
use iced::{Background, Border, Element, Length};

use crate::backend::{ApplicationChoice, FileHit};
use crate::icons::{Icon, Icons};
use crate::matcher;
use crate::model::{self, Command, Cursor, NUMBERED_SLOTS};
use crate::ui::{self, palette};

#[derive(Debug, Clone)]
pub enum Event {
    Query(String),
    Open(String),
    Reload,
    Back,
}

pub enum Effect {
    None,
    Focus,
    Close,
    Reload,
    Open(String),
}

enum State {
    Loading,
    Ready(Vec<ApplicationChoice>),
    Error(String),
}

pub struct Panel {
    pub file: FileHit,
    pub query: String,
    cursor: Cursor,
    state: State,
    icons: Vec<Option<Icon>>,
    error: Option<String>,
    restore: Option<String>,
}

pub fn input_id() -> Id {
    Id::new("open-with-search")
}

impl Panel {
    pub fn new(file: FileHit) -> Self {
        Self {
            file,
            query: String::new(),
            cursor: Cursor::new(6),
            state: State::Loading,
            icons: Vec::new(),
            error: None,
            restore: None,
        }
    }

    fn apps(&self) -> &[ApplicationChoice] {
        match &self.state {
            State::Ready(apps) => apps,
            _ => &[],
        }
    }

    fn filtered(&self) -> Vec<usize> {
        let tokens = matcher::tokens(&self.query);
        let mut matches: Vec<_> = self
            .apps()
            .iter()
            .enumerate()
            .filter_map(|(i, app)| {
                let name = matcher::fold(&app.name);
                let id = matcher::fold(&app.id);
                let score = if tokens.is_empty() {
                    Some(0)
                } else {
                    matcher::score(&tokens, &[(name.as_str(), 100), (id.as_str(), 50)])
                };
                score.map(|score| (i, score))
            })
            .collect();
        matches.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        matches.into_iter().map(|(i, _)| i).collect()
    }

    pub fn loading(&mut self) -> bool {
        if matches!(self.state, State::Loading) {
            return false;
        }
        if matches!(self.state, State::Ready(_)) {
            self.restore = self.filtered().get(self.cursor.selected).map(|&i| self.apps()[i].id.clone());
        }
        self.state = State::Loading;
        self.error = None;
        self.icons.clear();
        self.cursor.reset();
        true
    }

    pub fn loaded(&mut self, result: Result<Vec<ApplicationChoice>, String>, icons: &mut Icons) {
        match result {
            Ok(apps) => {
                self.icons = apps.iter().map(|app| icons.get(&app.icon, &["application-x-executable"])).collect();
                self.state = State::Ready(apps);
            }
            Err(error) => {
                self.state = State::Error(error);
                return;
            }
        }
        let filtered = self.filtered();
        let selected =
            self.restore.take().and_then(|id| filtered.iter().position(|&i| self.apps()[i].id == id)).unwrap_or(0);
        self.cursor.select(selected, filtered.len());
    }

    pub fn failed(&mut self, error: String) {
        self.error = Some(error);
        self.cursor.select(self.cursor.selected, self.filtered().len());
    }

    pub fn opening(&mut self) {
        self.error = None;
        self.cursor.select(self.cursor.selected, self.filtered().len());
    }

    pub fn event(&mut self, event: Event) -> Effect {
        match event {
            Event::Query(query) => {
                self.query = query;
                self.restore = None;
                self.cursor.reset();
            }
            Event::Reload if !matches!(self.state, State::Loading) => return Effect::Reload,
            Event::Back if self.query.is_empty() => return Effect::Close,
            Event::Back => {
                self.query.clear();
                self.restore = None;
                self.cursor.reset();
                return Effect::Focus;
            }
            Event::Open(id) => {
                let filtered = self.filtered();
                if let Some(index) = filtered.iter().position(|&i| self.apps()[i].id == id) {
                    self.cursor.select(index, filtered.len());
                    return Effect::Open(id);
                }
            }
            _ => {}
        }
        Effect::None
    }

    pub fn command(&mut self, cmd: Command) -> Effect {
        let filtered = self.filtered();
        let len = filtered.len();
        let page = self.cursor.visible.saturating_sub(1).max(1) as isize;
        match cmd {
            Command::Back => return self.event(Event::Back),
            Command::Activate => {
                if let Some(&i) = filtered.get(self.cursor.selected) {
                    return Effect::Open(self.apps()[i].id.clone());
                }
            }
            Command::ActivateVisible(slot) if slot < NUMBERED_SLOTS => {
                if let Some(i) = self.cursor.window(len).nth(slot) {
                    self.cursor.select(i, len);
                    return Effect::Open(self.apps()[filtered[i]].id.clone());
                }
            }
            Command::Up => self.cursor.move_by(-1, len),
            Command::Down => self.cursor.move_by(1, len),
            Command::PageUp => self.cursor.move_by(-page, len),
            Command::PageDown => self.cursor.move_by(page, len),
            Command::First => self.cursor.move_by(-(len as isize), len),
            Command::Last => self.cursor.move_by(len as isize, len),
            _ => {}
        }
        Effect::None
    }

    /// Fit rows after reserving the header, input, footer and feedback headroom.
    /// Error text can wrap.
    pub fn resize(&mut self, height: u32, opening: bool) {
        let reserved = 200
            + if self.error.is_some() {
                100
            } else if opening {
                20
            } else {
                0
            };
        self.cursor.visible = (height.saturating_sub(reserved) as usize / 47).clamp(1, 6);
        self.cursor.move_by(0, self.filtered().len());
    }

    pub fn wheel(&mut self, delta: isize) {
        self.cursor.move_by(delta, self.filtered().len());
    }

    pub fn view(&self, opening: bool) -> Element<'_, Event> {
        let mut reload = button(ui::hint("Ctrl+R", "Reload")).style(ui::status_button_style);
        if !opening && !matches!(self.state, State::Loading) {
            reload = reload.on_press(Event::Reload);
        }
        let title = row![text("Open with…").size(20), space::horizontal(), reload].align_y(Vertical::Center);
        let path =
            self.file.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| self.file.id.clone());
        let display = if self.file.path.is_some() { model::home_relative(&path) } else { path.clone() };
        let target = widget::responsive(move |size| {
            widget::tooltip(
                text(ui::label_for_width(&display, size.width, 12.0, true))
                    .size(12)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(palette::DIM.resolve(theme)),
                    })
                    .wrapping(text::Wrapping::None),
                container(text(path.clone()).size(12).wrapping(text::Wrapping::WordOrGlyph))
                    .padding(8)
                    .max_width(560)
                    .style(ui::surface_style),
                widget::tooltip::Position::Bottom,
            )
            .into()
        })
        .height(Length::Shrink);
        let mut input = text_input("Search applications…", &self.query)
            .id(input_id())
            .size(15)
            .padding([9, 12])
            .style(ui::field_style);
        if !opening {
            input = input.on_input(Event::Query);
        }
        let mut body = column![].spacing(3);
        match &self.state {
            State::Loading => {
                body = body.push(text("Loading applications…").size(16).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) }
                }))
            }
            State::Error(error) => {
                body = body.push(text("Applications could not be loaded").size(16));
                body = body.push(text(ui::ellipsize_end(error, 300)).size(13).style(move |theme: &iced::Theme| {
                    iced::widget::text::Style { color: Some(palette::ERROR.resolve(theme)) }
                }));
                body = body.push(text("Use Reload to try again, or Esc to return to search.").size(12).style(
                    move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)) },
                ));
            }
            State::Ready(apps) => {
                let filtered = self.filtered();
                if filtered.is_empty() {
                    body = body.push(
                        text(if apps.is_empty() {
                            "No compatible applications found"
                        } else {
                            "No matching applications"
                        })
                        .size(16),
                    );
                    body = body.push(text(if apps.is_empty() {
                        "Only apps declaring support for this file type appear. Return to search to use the desktop default."
                    } else { "Try another application name or clear the filter with Esc." }).size(13).style(move |theme: &iced::Theme| iced::widget::text::Style { color: Some(palette::DIM.resolve(theme)), }));
                }
                for (slot, row_index) in self.cursor.window(filtered.len()).enumerate() {
                    let i = filtered[row_index];
                    let app = &apps[i];
                    let selected = self.cursor.selected == row_index;
                    let icon: Element<'_, Event> = match self.icons.get(i).cloned().flatten() {
                        Some(Icon::Svg(handle)) => svg(handle).width(24).height(24).into(),
                        Some(Icon::Raster(handle)) => image(handle).width(24).height(24).into(),
                        None => space().width(24).height(24).into(),
                    };
                    let labels = widget::responsive(move |size| {
                        column![
                            text(ui::label_for_width(&app.name, size.width, 15.0, false))
                                .size(15)
                                .style(move |theme: &iced::Theme| iced::widget::text::Style {
                                    color: Some(if opening {
                                        palette::DIM.resolve(theme)
                                    } else {
                                        palette::TEXT.resolve(theme)
                                    }),
                                })
                                .wrapping(text::Wrapping::None),
                            text(ui::label_for_width(&app.id, size.width, 11.0, true))
                                .size(11)
                                .style(move |theme: &iced::Theme| iced::widget::text::Style {
                                    color: Some(palette::DIM.resolve(theme)),
                                })
                                .wrapping(text::Wrapping::None)
                        ]
                        .spacing(1)
                        .into()
                    })
                    .height(Length::Shrink);
                    let content = row![
                        ui::selection_bar(selected, 22.0),
                        row![
                            container(icon).width(28).align_x(Horizontal::Center),
                            container(labels).width(Length::Fill).clip(true),
                            ui::numbered_hint(slot, selected),
                        ]
                        .spacing(12)
                        .padding(iced::padding::left(9).right(16))
                        .align_y(Vertical::Center)
                    ]
                    .align_y(Vertical::Center);
                    let mut choice = button(container(content).height(Length::Fill).center_y(Length::Fill))
                        .height(44)
                        .width(Length::Fill)
                        .padding(0)
                        .style(move |theme, status| button::Style {
                            background: if selected {
                                Some(Background::Color(palette::SELECTED.resolve(theme)))
                            } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                                Some(Background::Color(palette::FAINT.resolve(theme)))
                            } else {
                                None
                            },
                            text_color: palette::TEXT.resolve(theme),
                            border: Border { radius: 9.0.into(), ..Default::default() },
                            ..Default::default()
                        });
                    if !opening {
                        choice = choice.on_press(Event::Open(app.id.clone()));
                    }
                    body = body.push(choice);
                }
            }
        }
        if let Some(error) = &self.error {
            body = body.push(text(ui::ellipsize_end(error, 220)).size(12).style(move |theme: &iced::Theme| {
                iced::widget::text::Style { color: Some(palette::ERROR.resolve(theme)) }
            }));
        }
        let back = button(ui::hint("Esc", if self.query.is_empty() { "Back to search" } else { "Clear filter" }))
            .on_press(Event::Back)
            .style(ui::status_button_style);
        let (status, color) = if opening {
            ("Opening…".to_string(), palette::ACCENT)
        } else if self.error.is_some() {
            ("Could not open file".into(), palette::ERROR)
        } else {
            match &self.state {
                State::Loading => ("Loading applications…".into(), palette::DIM),
                State::Error(_) => ("Applications unavailable".into(), palette::ERROR),
                State::Ready(_) => {
                    let filtered = self.filtered();
                    let window = self.cursor.window(filtered.len());
                    let status = if filtered.is_empty() {
                        "0 applications".into()
                    } else {
                        format!("{}–{} / {} applications", window.start + 1, window.end, filtered.len())
                    };
                    (status, palette::DIM)
                }
            }
        };
        let footer = row![
            text(status)
                .size(12)
                .width(Length::Fill)
                .wrapping(text::Wrapping::None)
                .style(move |theme: &iced::Theme| text::Style { color: Some(color.resolve(theme)) }),
            if !opening && !self.filtered().is_empty() {
                ui::primary_hint("Enter", "Open")
            } else {
                ui::hint("Enter", "Open")
            },
            back
        ]
        .spacing(14)
        .align_y(Vertical::Center);
        container(
            column![
                container(column![title, container(target).width(Length::Fill).clip(true)].spacing(7))
                    .padding([10, 16]),
                container(input).padding([0, 16]),
                container(body).padding([10, 0]).height(Length::Fill),
                container(footer)
                    .height(crate::layout::FOOTER_HEIGHT)
                    .padding(iced::padding::left(16).right(10))
                    .center_y(crate::layout::FOOTER_HEIGHT)
                    .style(ui::footer_style)
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
