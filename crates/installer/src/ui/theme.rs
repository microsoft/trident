//! Shares the terminal colour palette between logs, rendering and Linux VT setup.

use std::io;

use log::LevelFilter;
use ratatui::style::Color;

pub(super) const COLOR_BACKGROUND: Color = Color::Rgb(11, 17, 24);
pub(super) const COLOR_FRAME: Color = Color::Rgb(53, 69, 83);
pub(super) const COLOR_BRAND: Color = Color::Rgb(75, 216, 220);
pub(super) const COLOR_INST: Color = Color::Rgb(171, 186, 196);
pub(super) const COLOR_TRIDENT: Color = Color::Rgb(114, 185, 165);
pub(super) const COLOR_ERROR: Color = Color::Rgb(255, 107, 115);
pub(super) const COLOR_WARN: Color = Color::Rgb(245, 182, 93);
pub(super) const COLOR_INFO: Color = Color::Rgb(229, 234, 236);
pub(super) const COLOR_DEBUG: Color = Color::Rgb(167, 180, 189);
pub(super) const COLOR_TRACE: Color = Color::Rgb(99, 115, 129);
pub(super) const COLOR_SUCCESS: Color = Color::Rgb(93, 200, 148);
// Linux VT maps RGB SGR values into 16 colours, so give each UI role its own ANSI slot.
const VT_PALETTE: &[(u8, Color, Color)] = &[
    (0, COLOR_BACKGROUND, Color::Black),
    (1, COLOR_ERROR, Color::Red),
    (2, COLOR_TRIDENT, Color::Green),
    (3, COLOR_WARN, Color::Yellow),
    (4, COLOR_FRAME, Color::Blue),
    (5, COLOR_INST, Color::Magenta),
    (6, COLOR_BRAND, Color::Cyan),
    (7, COLOR_DEBUG, Color::Gray),
    (8, COLOR_TRACE, Color::DarkGray),
    (10, COLOR_SUCCESS, Color::LightGreen),
    (15, COLOR_INFO, Color::White),
];

/// Maps known theme RGB colors to distinct ANSI roles on Linux VTs.
/// Other colors and non-VT terminals retain the original value.
pub(super) fn theme_color(color: Color, linux_vt: bool) -> Color {
    if !linux_vt {
        return color;
    }
    VT_PALETTE
        .iter()
        .find(|(_, rgb, _)| *rgb == color)
        .map(|(_, _, ansi)| *ansi)
        .unwrap_or(color)
}

/// Builds the Linux VT palette entries, rejecting any theme slot without an RGB value.
pub(super) fn vt_rgb_palette() -> Result<Vec<(u8, u8, u8, u8)>, io::Error> {
    VT_PALETTE
        .iter()
        .map(|(slot, color, _)| match color {
            Color::Rgb(red, green, blue) => Ok((*slot, *red, *green, *blue)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Linux VT palette requires RGB values, got {other:?}"),
            )),
        })
        .collect()
}
pub(super) const VERBOSITY_LEVELS: [LevelFilter; 6] = [
    LevelFilter::Off,
    LevelFilter::Error,
    LevelFilter::Warn,
    LevelFilter::Info,
    LevelFilter::Debug,
    LevelFilter::Trace,
];
