//! Terminal color detection for user-facing output.
//!
//! A single, process-global decision shared by tailor's own status output and the pass-through of
//! Image Customizer's (colored) logs, so color is preserved or suppressed consistently.

use std::{io::IsTerminal as _, sync::OnceLock};

/// Whether to emit/preserve ANSI color on stderr: honor `NO_COLOR` (force off) and `CLICOLOR_FORCE`
/// (force on), otherwise color only when stderr is a terminal. Computed once.
pub fn color_enabled() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            false
        } else if std::env::var_os("CLICOLOR_FORCE").is_some() {
            true
        } else {
            std::io::stderr().is_terminal()
        }
    })
}
