//! Shares the terminal colour palette between logs, rendering and Linux VT setup.

use std::io;

use log::LevelFilter;
use ratatui::style::Color;

/// Keep the same dark surface across Ratatui panels and the Linux VT palette.
pub(super) const COLOR_BACKGROUND: Color = Color::Rgb(11, 17, 24);
/// Neutral borders distinguish ordinary panels from success and failure.
pub(super) const COLOR_FRAME: Color = Color::Rgb(53, 69, 83);
/// Highlight the wordmark, selected controls and current activity.
pub(super) const COLOR_BRAND: Color = Color::Rgb(75, 216, 220);
/// Source tag for messages produced by the installer itself.
pub(super) const COLOR_INST: Color = Color::Rgb(171, 186, 196);
/// Source tag for logs received from the Trident daemon.
pub(super) const COLOR_TRIDENT: Color = Color::Rgb(114, 185, 165);
/// Failure frame and error severity share one palette role.
pub(super) const COLOR_ERROR: Color = Color::Rgb(255, 107, 115);
/// Unknown daemon levels receive the same attention as warnings.
pub(super) const COLOR_WARN: Color = Color::Rgb(245, 182, 93);
/// Informational records stay readable against the dark background.
pub(super) const COLOR_INFO: Color = Color::Rgb(229, 234, 236);
/// Debug records remain legible without competing with results.
pub(super) const COLOR_DEBUG: Color = Color::Rgb(167, 180, 189);
/// Trace records are the least prominent diagnostic level.
pub(super) const COLOR_TRACE: Color = Color::Rgb(99, 115, 129);
/// Successful completion uses a distinct result color.
pub(super) const COLOR_SUCCESS: Color = Color::Rgb(93, 200, 148);
/// Linux VT maps RGB requests into 16 ANSI colors, so each role needs its own slot.
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

/// One stable order for the picker and its number-key shortcuts.
pub(super) const VERBOSITY_LEVELS: [LevelFilter; 6] = [
    LevelFilter::Off,
    LevelFilter::Error,
    LevelFilter::Warn,
    LevelFilter::Info,
    LevelFilter::Debug,
    LevelFilter::Trace,
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
