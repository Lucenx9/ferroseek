// SPDX-License-Identifier: GPL-3.0-or-later
//! A secondary application chooser bound to one immutable file result.
use iced::alignment::Vertical;
use iced::widget::{self, Id, button, column, container, image, row, space, svg, text, text_input};
use iced::{Background, Border, Element, Length};

use crate::backend::{ApplicationChoice, FileHit};
use crate::icons::{Icon, Icons};
use crate::matcher;
use crate::model::{Command, Cursor, NUMBERED_SLOTS};
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
                let name = app.name.to_lowercase();
                let id = app.id.to_lowercase();
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
        self.restore = self.filtered().get(self.cursor.selected).map(|&i| self.apps()[i].id.clone());
        self.state = State::Loading;
        self.error = None;
        self.icons.clear();
        self.cursor.reset();
        self.cursor.visible = 6;
        true
    }

    pub fn loaded(&mut self, result: Result<Vec<ApplicationChoice>, String>, icons: &mut Icons) {
        self.state = match result {
            Ok(apps) => {
                self.icons = apps.iter().map(|app| icons.get(&app.icon, &["application-x-executable"])).collect();
                State::Ready(apps)
            }
            Err(error) => State::Error(error),
        };
        let filtered = self.filtered();
        let selected =
            self.restore.take().and_then(|id| filtered.iter().position(|&i| self.apps()[i].id == id)).unwrap_or(0);
        self.cursor.select(selected, filtered.len());
    }

    pub fn failed(&mut self, error: String) {
        self.error = Some(error);
        self.cursor.visible = 4;
        self.cursor.select(self.cursor.selected, self.filtered().len());
    }

    pub fn opening(&mut self) {
        self.error = None;
        self.cursor.visible = 6;
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
        match cmd {
            Command::Back => return self.event(Event::Back),
            Command::Activate => {
                if let Some(&i) = filtered.get(self.cursor.selected) {
                    return Effect::Open(self.apps()[i].id.clone());
                }
            }
            Command::ActivateVisible(slot) if slot < NUMBERED_SLOTS => {
                if let Some(i) = self.cursor.window(len).nth(slot) {
                    return Effect::Open(self.apps()[filtered[i]].id.clone());
                }
            }
            Command::Up => self.cursor.move_by(-1, len),
            Command::Down => self.cursor.move_by(1, len),
            Command::PageUp => self.cursor.move_by(-(self.cursor.visible as isize - 1), len),
            Command::PageDown => self.cursor.move_by(self.cursor.visible as isize - 1, len),
            Command::First => self.cursor.move_by(-(len as isize), len),
            Command::Last => self.cursor.move_by(len as isize, len),
            _ => {}
        }
        Effect::None
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
        let path = self
            .file
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file.subtext.clone());
        let target = text(ui::ellipsize_middle(&path, 88)).size(12).color(palette::DIM).wrapping(text::Wrapping::None);
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
            State::Loading => body = body.push(text("Loading applications…").size(16).color(palette::DIM)),
            State::Error(error) => {
                body = body.push(text("Applications could not be loaded").size(16));
                body = body.push(text(ui::ellipsize_end(error, 300)).size(13).color(palette::ERROR));
                body = body
                    .push(text("Use Reload to try again, or Esc to return to search.").size(12).color(palette::DIM));
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
                    } else { "Try another application name or clear the filter with Esc." }).size(13).color(palette::DIM));
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
                    let labels = column![
                        text(ui::ellipsize_end(&app.name, 52))
                            .size(15)
                            .color(if opening { palette::DIM } else { palette::TEXT })
                            .wrapping(text::Wrapping::None),
                        text(ui::ellipsize_middle(&app.id, 72))
                            .size(11)
                            .color(palette::DIM)
                            .wrapping(text::Wrapping::None)
                    ]
                    .spacing(1);
                    let content = row![
                        icon,
                        container(labels).width(Length::Fill).clip(true),
                        text(format!("Ctrl+{}", slot + 1)).size(11).color(palette::DIM)
                    ]
                    .spacing(12)
                    .align_y(Vertical::Center);
                    let mut choice = button(container(content).height(Length::Fill).center_y(Length::Fill))
                        .height(48)
                        .width(Length::Fill)
                        .padding([0, 16])
                        .style(move |_, status| button::Style {
                            background: if selected {
                                Some(Background::Color(palette::SELECTED))
                            } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                                Some(Background::Color(palette::FAINT))
                            } else {
                                None
                            },
                            text_color: palette::TEXT,
                            border: Border { radius: 9.0.into(), ..Default::default() },
                            ..Default::default()
                        });
                    if !opening {
                        choice = choice.on_press(Event::Open(app.id.clone()));
                    }
                    body = body.push(choice);
                }
                if !filtered.is_empty() {
                    let window = self.cursor.window(filtered.len());
                    body = body.push(
                        text(format!("{}–{} / {} applications", window.start + 1, window.end, filtered.len()))
                            .size(11)
                            .color(palette::DIM),
                    );
                }
            }
        }
        if let Some(error) = &self.error {
            body = body.push(text(ui::ellipsize_end(error, 220)).size(12).color(palette::ERROR));
        }
        if opening {
            body = body.push(text("Opening the selected file…").size(12).color(palette::ACCENT));
        }
        let back = button(ui::hint("Esc", if self.query.is_empty() { "Back to search" } else { "Clear filter" }))
            .on_press(Event::Back)
            .style(ui::status_button_style);
        let footer = row![back, space::horizontal(), ui::hint("Enter", "Open")].align_y(Vertical::Center);
        container(
            column![
                container(column![title, target].spacing(7)).padding([10, 16]),
                container(input).padding([0, 16]),
                container(body).padding([10, 8]).height(Length::Fill),
                container(footer).height(34).padding([0, 10]).center_y(34)
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
