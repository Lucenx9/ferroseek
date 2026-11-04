// SPDX-License-Identifier: GPL-3.0-or-later
//! Logical surface geometry. Saved presets remain independent of output limits.
use ferroseek_config::appearance::{Position, Preferences};
pub const ROW_HEIGHT: f32 = 48.0;
pub const INPUT_HEIGHT: f32 = 58.0;
pub const FOOTER_HEIGHT: f32 = 34.0;
pub const PAD: f32 = 8.0;
const EDGE: u32 = 16;
const PREVIEW_EXTRA: u32 = 260;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub rows: usize,
    pub header_extra: u32,
    pub centered: bool,
    pub top: u32,
}
pub fn calculate(preferences: Preferences, bounds: Option<(u32, u32)>, compact: bool, preview: bool) -> Geometry {
    let desired = preferences.width.pixels() + if preview { PREVIEW_EXTRA } else { 0 };
    let width = bounds.map_or(desired, |(w, _)| desired.min(w.saturating_sub(2 * EDGE).max(1)));
    let header_extra = if width < 640 { 30 } else { 0 };
    let base = (PAD * 2.0 + INPUT_HEIGHT + 1.0 + FOOTER_HEIGHT) as u32 + header_extra;
    let available = bounds.map_or(u32::MAX, |(_, h)| h.saturating_sub(2 * EDGE).max(1));
    let rows = ((available.saturating_sub(base + PAD as u32) / (ROW_HEIGHT as u32)) as usize)
        .clamp(1, crate::model::NUMBERED_SLOTS);
    let full = base + ROW_HEIGHT as u32 * rows as u32 + PAD as u32;
    let height = (if compact { base } else { full }).min(available);
    let centered = preferences.position == Position::Centered;
    let top = if centered { 0 } else { bounds.map_or(200, |(_, h)| 200.min(h.saturating_sub(full + EDGE).max(EDGE))) };
    Geometry { width, height, rows, header_extra, centered, top }
}
