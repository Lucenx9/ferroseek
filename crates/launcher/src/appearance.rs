// SPDX-License-Identifier: GPL-3.0-or-later
//! Appearance preferences, shared semantic colors and their settings view.
use ferroseek_config::appearance::{Palette, Preferences};
use iced::{Color, Theme};

pub fn theme(preferences: Preferences, system: Option<bool>, glass: bool) -> Theme {
    let dark = preferences.dark(system);
    let choice = if dark { preferences.dark_theme } else { preferences.light_theme };
    let (surface, text, accent) = match (choice, dark) {
        (Palette::Iron, true) => (0x17181c, 0xeceae6, 0xe0703a),
        (Palette::Iron, false) => (0xf4f2ee, 0x25272c, 0x99421e),
        (Palette::Slate, true) => (0x161d27, 0xe8edf4, 0x79ace5),
        (Palette::Slate, false) => (0xeff3f8, 0x202d40, 0x285a98),
        (Palette::Forest, true) => (0x18221e, 0xe7eee8, 0x83bea2),
        (Palette::Forest, false) => (0xf0f5f0, 0x22382c, 0x286246),
    };
    let mut base = if dark { Theme::Dark.palette() } else { Theme::Light.palette() };
    base.background = rgb(surface);
    if glass {
        base.background.a = 0.88;
    }
    base.text = rgb(text);
    base.primary = rgb(accent);
    base.danger = rgb(if dark { 0xe86a6a } else { 0xa92d2d });
    Theme::custom("FerroSeek", base)
}
fn rgb(value: u32) -> Color {
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

pub mod palette {
    use iced::{Color, Theme};
    #[derive(Clone, Copy)]
    pub enum Role {
        Surface,
        Border,
        Text,
        Dim,
        Faint,
        Accent,
        Selected,
        Error,
        Raised,
    }
    pub const SURFACE: Role = Role::Surface;
    pub const BORDER: Role = Role::Border;
    pub const TEXT: Role = Role::Text;
    pub const DIM: Role = Role::Dim;
    pub const FAINT: Role = Role::Faint;
    pub const ACCENT: Role = Role::Accent;
    pub const SELECTED: Role = Role::Selected;
    pub const ERROR: Role = Role::Error;
    pub const RAISED: Role = Role::Raised;
    impl Role {
        pub fn resolve(self, theme: &Theme) -> Color {
            let p = theme.palette();
            let dark = p.text.r > p.background.r;
            let alpha = |mut color: Color, a| {
                color.a = a;
                color
            };
            match self {
                Self::Surface => alpha(p.background, 1.0),
                Self::Border => alpha(p.text, 0.18),
                Self::Text => p.text,
                Self::Dim => super::rgb(if dark { 0xc0c2ca } else { 0x434852 }),
                Self::Faint => alpha(p.text, 0.06),
                Self::Accent => p.primary,
                Self::Selected => alpha(p.primary, 0.18),
                Self::Error => p.danger,
                Self::Raised => {
                    let shift = |value: f32, amount: f32| (value + amount / 255.0).clamp(0.0, 1.0);
                    Color::from_rgb(
                        shift(p.background.r, if dark { 11.0 } else { -8.0 }),
                        shift(p.background.g, if dark { 11.0 } else { -8.0 }),
                        shift(p.background.b, if dark { 13.0 } else { -8.0 }),
                    )
                }
            }
        }
    }
}

use crate::{
    model::{Command, Cursor},
    ui,
};
use ferroseek_config::appearance::{Material, Mode, Position, Width};
use ferroseek_config::folders::Document;
use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{Background, Border, Element, Font, Length, Task};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Colors,
    Layout,
}

#[derive(Debug, Clone)]
pub enum Event {
    Tab(Tab),
    Loaded(Result<Document, String>),
    Choose(usize),
    Save,
    Saved(Result<Document, String>),
    Back,
    Discard,
    Keep,
    Reload,
}
pub struct Panel {
    tab: Tab,
    path: PathBuf,
    pub draft: Preferences,
    original: Preferences,
    document: Option<Document>,
    cursor: Cursor,
    busy: bool,
    confirm: bool,
    notice: String,
}
impl Panel {
    pub fn new(path: PathBuf, current: Preferences) -> Self {
        Self {
            tab: Tab::Colors,
            path,
            draft: current,
            original: current,
            document: None,
            cursor: Cursor::new(13),
            busy: true,
            confirm: false,
            notice: "Loading appearance…".into(),
        }
    }
    pub fn load(&self) -> Task<Event> {
        let path = self.path.clone();
        Task::perform(async move { zruntime::unblock(move || Document::load(path)).await }, Event::Loaded)
    }
    pub fn content_id(&self) -> iced::widget::Id {
        iced::widget::Id::new(match self.tab {
            Tab::Colors => "appearance-colors",
            Tab::Layout => "appearance-layout",
        })
    }
    fn dirty(&self) -> bool {
        self.draft != self.original
    }
    /// Returns preferences to restore when leaving. Saved preferences stay live.
    pub fn update(&mut self, event: Event) -> (Task<Event>, Option<Preferences>) {
        match event {
            Event::Tab(tab) if !self.confirm => {
                self.tab = tab;
                self.cursor.reset();
            }

            Event::Loaded(result) => {
                self.busy = false;
                match result {
                    Ok(doc) => {
                        self.original = doc.config.appearance;
                        self.draft = self.original;
                        self.document = Some(doc);
                        self.notice.clear();
                    }
                    Err(error) => self.notice = error,
                }
            }
            Event::Saved(result) => {
                self.busy = false;
                match result {
                    Ok(doc) => {
                        self.original = doc.config.appearance;
                        self.document = Some(doc);
                        self.notice = "Appearance saved".into();
                    }
                    Err(error) => self.notice = error,
                }
            }
            Event::Choose(index) if !self.busy && self.document.is_some() && !self.confirm => {
                self.cursor.select(index, self.controls());
                if self.tab == Tab::Layout {
                    match index {
                        0 => self.draft.width = Width::Compact,
                        1 => self.draft.width = Width::Standard,
                        2 => self.draft.width = Width::Wide,
                        3 => self.draft.position = Position::Upper,
                        4 => self.draft.position = Position::Centered,
                        5 => {
                            self.draft.width = Width::Standard;
                            self.draft.position = Position::Upper;
                        }
                        6 => return self.update(Event::Save),
                        7 => return self.update(Event::Back),
                        _ => {}
                    }
                    self.notice =
                        if index == 5 { "Default layout previewed. Save to keep it.".into() } else { String::new() };
                    return (Task::none(), None);
                }
                match index {
                    0 => self.draft.mode = Mode::System,
                    1 => self.draft.mode = Mode::Light,
                    2 => self.draft.mode = Mode::Dark,
                    3 => self.draft.material = Material::Solid,
                    4 => self.draft.material = Material::Glass,
                    5..=7 => self.draft.light_theme = PALETTES[index - 5].0,
                    8..=10 => self.draft.dark_theme = PALETTES[index - 8].0,
                    11 => return self.update(Event::Save),
                    12 => return self.update(Event::Back),
                    _ => {}
                }
                self.notice.clear();
            }
            Event::Save if !self.busy && !self.confirm => {
                if let Some(doc) = self.document.clone() {
                    self.busy = true;
                    let draft = self.draft;
                    return (
                        Task::perform(
                            async move { zruntime::unblock(move || doc.save_appearance(draft)).await },
                            Event::Saved,
                        ),
                        None,
                    );
                }
            }
            Event::Back if !self.busy => {
                if self.dirty() {
                    self.confirm = true;
                } else {
                    return (Task::none(), Some(self.original));
                }
            }
            Event::Discard if !self.busy && self.confirm => return (Task::none(), Some(self.original)),
            Event::Keep => self.confirm = false,
            Event::Reload if !self.busy && !self.dirty() => {
                self.busy = true;
                self.document = None;
                self.notice = "Loading appearance…".into();
                return (self.load(), None);
            }
            Event::Reload if self.dirty() => self.notice = "Save or discard changes before reloading.".into(),
            _ => {}
        }
        (Task::none(), None)
    }
    fn controls(&self) -> usize {
        if self.tab == Tab::Colors { 13 } else { 8 }
    }
    fn save_index(&self) -> usize {
        self.controls() - 2
    }
    pub fn command(&mut self, command: Command) -> (Task<Event>, Option<Preferences>) {
        if self.confirm {
            return match command {
                Command::Back => self.update(Event::Keep),
                _ => (Task::none(), None),
            };
        }
        if self.busy {
            return (Task::none(), None);
        }
        let controls = self.controls();
        match command {
            Command::PreviousScope | Command::NextScope => {
                return self.update(Event::Tab(if self.tab == Tab::Colors { Tab::Layout } else { Tab::Colors }));
            }
            Command::Up => self.cursor.move_by(-1, controls),
            Command::Down => self.cursor.move_by(1, controls),
            Command::First => self.cursor.move_by(-(controls as isize), controls),
            Command::Last => self.cursor.move_by(controls as isize, controls),
            Command::Activate => return self.update(Event::Choose(self.cursor.selected)),
            Command::Back => return self.update(Event::Back),
            _ => {}
        }
        (
            iced::widget::operation::snap_to(
                iced::widget::Id::new("appearance-scroll"),
                iced::widget::operation::RelativeOffset {
                    x: 0.0,
                    y: if self.tab == Tab::Colors && self.cursor.selected >= 8 { 1.0 } else { 0.0 },
                },
            ),
            None,
        )
    }
    pub fn view(&self, system: Option<bool>, opaque: bool, glass: bool) -> Element<'_, Event> {
        let enabled = !self.busy && self.document.is_some() && !self.confirm;
        let choice = |index, label: &'static str, selected| {
            let focused = self.cursor.selected == index;
            let b = button(choice_label(label, 13, selected))
                .width(Length::Fill)
                .padding([7, 10])
                .style(move |theme: &Theme, status| choice_style(theme, status, selected, focused));
            if enabled { b.on_press(Event::Choose(index)) } else { b }
        };
        let label = |value: &'static str| {
            text(value).size(12).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT })
        };
        let content = if self.tab == Tab::Layout {
            column![
                column![
                    row![label("Window width"), space::horizontal(), ui::hint("Ctrl+Tab", "Colors / Layout")],
                    row![
                        choice(0, "Compact · 640", self.draft.width == Width::Compact),
                        choice(1, "Standard · 720", self.draft.width == Width::Standard),
                        choice(2, "Wide · 880", self.draft.width == Width::Wide)
                    ]
                    .spacing(8),
                    text("Width is limited to the current screen. The side preview adapts to the available space.")
                        .size(12)
                        .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
                ]
                .spacing(6),
                column![
                    label("Position"),
                    row![
                        choice(3, "Upper", self.draft.position == Position::Upper),
                        choice(4, "Centered", self.draft.position == Position::Centered),
                        space::horizontal()
                    ]
                    .spacing(8),
                ]
                .spacing(6),
                column![
                    choice(5, "Restore default layout", false),
                    text("Restores width and position. Your themes and surface preference stay the same.")
                        .size(12)
                        .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
                ]
                .spacing(6),
            ]
            .spacing(16)
        } else {
            let mut surface = column![
                label("Surface"),
                row![
                    choice(3, "Solid", self.draft.material == Material::Solid),
                    choice(4, "Glass", self.draft.material == Material::Glass),
                    space::horizontal()
                ]
                .spacing(8),
            ]
            .spacing(6);
            let support = if opaque {
                Some("Solid for this session (--opaque)")
            } else if self.draft.material == Material::Glass && !glass {
                Some("Glass uses a solid surface when blur or transparency is unavailable")
            } else if self.draft.mode == Mode::System && system.is_none() {
                Some("No system color preference · using Dark")
            } else {
                None
            };
            if let Some(support) = support {
                surface = surface.push(
                    text(support)
                        .size(11)
                        .style(|theme: &Theme| text::Style { color: Some(palette::DIM.resolve(theme)) }),
                );
            }
            let mut content = column![
                column![
                    label("Color mode"),
                    row![
                        choice(0, "System", self.draft.mode == Mode::System),
                        choice(1, "Light", self.draft.mode == Mode::Light),
                        choice(2, "Dark", self.draft.mode == Mode::Dark)
                    ]
                    .spacing(8),
                ]
                .spacing(6),
                surface,
            ]
            .spacing(16);
            for dark in [false, true] {
                let mut cards = row![].spacing(8);
                for (i, (palette, name)) in PALETTES.iter().enumerate() {
                    let index = if dark { 8 + i } else { 5 + i };
                    let selected = if dark { self.draft.dark_theme } else { self.draft.light_theme } == *palette;
                    let focused = self.cursor.selected == index;
                    let mut preview = self.draft;
                    preview.mode = if dark { Mode::Dark } else { Mode::Light };
                    preview.light_theme = *palette;
                    preview.dark_theme = *palette;
                    let color_theme = theme(preview, system, false);
                    let b = button(column![miniature(&color_theme), choice_label(name, 12, selected)].spacing(4))
                        .padding(5)
                        .width(Length::Fill)
                        .style(move |theme: &Theme, status| choice_style(theme, status, selected, focused));
                    cards = cards.push(if enabled { b.on_press(Event::Choose(index)) } else { b });
                }
                content =
                    content.push(column![label(if dark { "Dark theme" } else { "Light theme" }), cards].spacing(6));
            }
            content
        };
        let save = button(ui::hint("Ctrl+S", "Save"))
            .style(move |theme, status| choice_style(theme, status, false, self.cursor.selected == self.save_index()));
        let back = button(ui::hint("Esc", "Back")).style(move |theme, status| {
            choice_style(theme, status, false, self.cursor.selected == self.save_index() + 1)
        });
        let footer = if self.confirm {
            row![
                text("Discard changes?").size(13),
                space::horizontal(),
                button(ui::hint("Esc", "Keep editing")).on_press(Event::Keep),
                button(ui::hint("Ctrl+D", "Discard")).on_press(Event::Discard)
            ]
            .spacing(12)
            .align_y(iced::Center)
        } else {
            row![
                text(if self.dirty() { "Unsaved changes" } else { "Preview changes immediately · Save to keep them" })
                    .size(11),
                space::horizontal(),
                if enabled { save.on_press(Event::Save) } else { save },
                if self.busy { back } else { back.on_press(Event::Back) }
            ]
            .spacing(12)
            .align_y(iced::Center)
        };
        let notice: Element<'_, Event> = if self.notice.is_empty() {
            space().height(0).into()
        } else {
            container(text(ui::ellipsize_end(&self.notice, 220)).size(12)).padding([6, 24]).into()
        };
        container(column![
            row![
                text("Appearance").size(20),
                button(text("Colors").size(12))
                    .on_press(Event::Tab(Tab::Colors))
                    .style(move |theme, status| choice_style(theme, status, self.tab == Tab::Colors, false)),
                button(text("Layout").size(12))
                    .on_press(Event::Tab(Tab::Layout))
                    .style(move |theme, status| choice_style(theme, status, self.tab == Tab::Layout, false)),
                space::horizontal(),
                button(ui::hint("Ctrl+R", "Reload")).on_press(Event::Reload).style(ui::status_button_style)
            ]
            .spacing(8)
            .padding([8, 24])
            .align_y(iced::Center),
            ui::divider(),
            notice,
            scrollable(content.padding([12, 24])).id(iced::widget::Id::new("appearance-scroll")).height(Length::Fill),
            ui::divider(),
            container(footer).padding([8, 24])
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(ui::surface_style)
        .into()
    }
}
const PALETTES: [(Palette, &str); 3] =
    [(Palette::Iron, "Iron"), (Palette::Slate, "Slate"), (Palette::Forest, "Forest")];

fn choice_label(label: &str, size: u32, selected: bool) -> Element<'_, Event> {
    row![
        text(label).size(size).width(Length::Fill),
        text(if selected { "✓" } else { "" }).size(size).width(16).align_x(iced::Right),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .into()
}

fn choice_style(theme: &Theme, status: button::Status, selected: bool, focused: bool) -> button::Style {
    button::Style {
        background: Some(Background::Color(if selected || status == button::Status::Pressed {
            palette::SELECTED.resolve(theme)
        } else {
            palette::FAINT.resolve(theme)
        })),
        text_color: palette::TEXT.resolve(theme),
        border: Border {
            color: if focused {
                palette::ACCENT.resolve(theme)
            } else if status == button::Status::Hovered {
                Color { a: 0.32, ..palette::TEXT.resolve(theme) }
            } else {
                palette::BORDER.resolve(theme)
            },
            width: if focused { 2.0 } else { 1.0 },
            radius: 8.0.into(),
        },
        ..button::Style::default()
    }
}
/// A small functional sample composed from the launcher's text and surface roles.
fn miniature(theme: &Theme) -> Element<'static, Event> {
    let surface = palette::SURFACE.resolve(theme);
    let selected = palette::SELECTED.resolve(theme);
    let foreground = palette::TEXT.resolve(theme);
    let dim = palette::DIM.resolve(theme);
    let accent = palette::ACCENT.resolve(theme);
    let bar = container(space()).width(3).height(11).style(move |_| container::Style {
        background: Some(accent.into()),
        border: Border { radius: 1.5.into(), ..Default::default() },
        ..Default::default()
    });
    container(
        column![
            text("Search…").size(11).color(dim),
            container(row![bar, text("Files").size(11).color(foreground)].spacing(4).align_y(iced::Center))
                .padding([2, 5])
                .width(Length::Fill)
                .style(move |_| {
                    container::Style {
                        background: Some(selected.into()),
                        border: Border { radius: 4.0.into(), ..Default::default() },
                        ..Default::default()
                    }
                }),
            text("~/Documents").size(10).color(dim)
        ]
        .spacing(3),
    )
    .padding(6)
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(surface.into()),
        border: Border { radius: 5.0.into(), ..Default::default() },
        ..Default::default()
    })
    .into()
}
