// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent color mode, material and preferred light/dark palettes.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    Solid,
    #[default]
    Glass,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    #[default]
    Iron,
    Slate,
    Forest,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Width {
    Compact,
    #[default]
    Standard,
    Wide,
}
impl Width {
    pub fn pixels(self) -> u32 {
        match self {
            Self::Compact => 640,
            Self::Standard => 720,
            Self::Wide => 880,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    Upper,
    Centered,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub mode: Mode,
    pub material: Material,
    pub light_theme: Palette,
    pub dark_theme: Palette,
    pub width: Width,
    pub position: Position,
}
impl Preferences {
    /// Missing/unknown desktop preference keeps the launcher's dark default.
    pub fn dark(self, system: Option<bool>) -> bool {
        match self.mode {
            Mode::System => system.unwrap_or(true),
            Mode::Light => false,
            Mode::Dark => true,
        }
    }
}
