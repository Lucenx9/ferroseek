// SPDX-License-Identifier: GPL-3.0-or-later
//! Entry point to launcher preferences and file search configuration.

use iced::widget::operation::{self, RelativeOffset};
use iced::widget::{button, column, container, row, scrollable, space, text, tooltip};
use iced::{Element, Length, Task, Theme};

use super::{
    Launcher, Message, divider, footer_style, hint, keycap, page_header, palette, search_id, status_button_style,
    surface_style, tab_style,
};
use crate::commands::Builtin;
use crate::layout::{FOOTER_HEIGHT, PAD};
use crate::model::{ClickTracker, Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Appearance,
    SearchFolders,
    SearchStatus,
}

const ENTRIES: [Entry; 3] = [Entry::Appearance, Entry::SearchFolders, Entry::SearchStatus];
const OPEN_SHORTCUT: &str = "Ctrl+Shift+S";
const SCROLL_ID: &str = "settings-scroll";
pub(super) const HEIGHT: u32 = 320;

impl Entry {
    fn message(self) -> Message {
        match self {
            Self::Appearance => Message::ShowAppearance,
            Self::SearchFolders => Message::ShowFolders,
            Self::SearchStatus => Message::ShowSearchStatus,
        }
    }

    fn item(self, selected: bool) -> Element<'static, Message> {
        let (name, help, shortcut) = match self {
            Self::Appearance => (Builtin::Appearance.title(), "Theme, colors and layout", "Ctrl+Shift+,"),
            Self::SearchFolders => (Builtin::SearchFolders.title(), Builtin::SearchFolders.description(), "Ctrl+,"),
            Self::SearchStatus => (Builtin::SearchStatus.title(), Builtin::SearchStatus.description(), "Ctrl+I"),
        };
        button(
            row![column![text(name).size(15), dim(help)].spacing(2), space::horizontal(), keycap(shortcut, selected)]
                .align_y(iced::Center)
                .spacing(6),
        )
        .width(Length::Fill)
        .padding([4, 10])
        .on_press(Message::SettingsSelected(self))
        .style(move |theme, status| tab_style(theme, status, selected))
        .into()
    }
}

impl Launcher {
    pub(super) fn toggle_settings(&mut self) -> Task<Message> {
        if self.settings_menu_open() {
            self.settings = None;
        } else {
            if self.search_suspended() || self.opening.is_some() {
                return Task::none();
            }
            self.settings = Some(Entry::Appearance);
            self.panel = None;
            self.clicks = ClickTracker::default();
        }
        self.return_to_parent()
    }

    pub(super) fn settings_menu_open(&self) -> bool {
        self.settings.is_some()
            && self.appearance.is_none()
            && self.folder_settings.is_none()
            && self.search_status.is_none()
    }

    pub(super) fn return_to_parent(&mut self) -> Task<Message> {
        let resize = self.resize();
        let focus = self.settings.map_or_else(|| crate::input::focus(search_id()), scroll_to_entry);
        Task::batch([resize, focus])
    }

    pub(super) fn choose_setting(&mut self, entry: Entry) -> Task<Message> {
        if !self.settings_menu_open() {
            return Task::none();
        }
        super::update(self, entry.message())
    }

    pub(super) fn settings_command(&mut self, current: Entry, command: Command) -> Task<Message> {
        let index = current as usize;
        let next = match command {
            Command::Back => return self.toggle_settings(),
            Command::Activate => return self.choose_setting(current),
            Command::Up => index.saturating_sub(1),
            Command::Down => (index + 1).min(ENTRIES.len() - 1),
            Command::First => 0,
            Command::Last => ENTRIES.len() - 1,
            _ => return Task::none(),
        };
        let selected = ENTRIES[next];
        self.settings = Some(selected);
        scroll_to_entry(selected)
    }
}

fn scroll_to_entry(entry: Entry) -> Task<Message> {
    let index = entry as usize;
    operation::snap_to(SCROLL_ID, RelativeOffset { x: 0.0, y: index as f32 / (ENTRIES.len() - 1) as f32 })
}

fn dim(label: &'static str) -> iced::widget::Text<'static> {
    text(label).size(12).style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) })
}

pub(super) fn header_button() -> Element<'static, Message> {
    tooltip(
        button(text("Settings").size(12))
            .on_press(Message::ShowSettings)
            .padding([4, 8])
            .height(24)
            .style(status_button_style),
        container(keycap(OPEN_SHORTCUT, true)).padding(4).style(surface_style),
        tooltip::Position::Bottom,
    )
    .into()
}

pub(super) fn view(selected: Entry) -> Element<'static, Message> {
    let header = page_header(text("Settings").size(20)).height(56);
    let entries = |section: &'static str, items: &[Entry]| {
        items.iter().fold(column![container(dim(section)).padding([0, 10])].spacing(4), |column, &entry| {
            column.push(entry.item(entry == selected))
        })
    };
    // A distinct widget tree keeps this scroll position out of the child pages.
    let body = column![
        scrollable(
            column![entries("Launcher", &ENTRIES[..1]), entries("File search", &ENTRIES[1..])].spacing(8).padding(8),
        )
        .id(SCROLL_ID)
        .height(Length::Fill),
    ]
    .height(Length::Fill);
    let back = button(hint("Esc", "Back to search"))
        .on_press(Message::ShowSettings)
        .padding([4, 8])
        .style(status_button_style);
    let open = button(hint("Enter", "Open"))
        .on_press(Message::SettingsSelected(selected))
        .padding([4, 8])
        .style(status_button_style);
    let footer = container(row![back, space::horizontal(), open].align_y(iced::Center))
        .height(FOOTER_HEIGHT)
        .padding([0.0, PAD])
        .align_y(iced::Center)
        .style(footer_style);
    container(column![header, divider(), body, divider(), footer])
        .padding(PAD)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(surface_style)
        .into()
}
