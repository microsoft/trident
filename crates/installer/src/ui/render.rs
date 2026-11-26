//! Draws the Ratatui screens, their action lists and the scrollable log panel.

use std::{borrow::Cow, ops::Range, time::Instant};

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::Set as BorderSet,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use textwrap::{self, Options, WordSeparator, WordSplitter, WrapAlgorithm};

use crate::{diagnostics, APPLICATION_NAME};

use super::{
    model::{self, Model, Screen},
    theme::{
        self, COLOR_BACKGROUND, COLOR_BRAND, COLOR_ERROR, COLOR_FRAME, COLOR_SUCCESS,
        VERBOSITY_LEVELS,
    },
};

/// Reserve space for the wordmark, name and state on a standard terminal.
const WORDMARK_HEADER_HEIGHT: u16 = 12;
/// Keep a readable two-line heading when the wordmark does not fit.
const COMPACT_HEADER_HEIGHT: u16 = 2;
/// Leave enough height for logs and actions beneath the full wordmark.
const MIN_WORDMARK_TERMINAL_HEIGHT: u16 = 24;
/// Leave at least one content row inside the panel borders.
const MIN_CONTENT_HEIGHT: u16 = 3;
/// Keep key hints and elapsed time below the action list.
const FOOTER_HEIGHT: u16 = 2;
/// Trident name drawn without relying on terminal-specific fonts or graphics.
const TRIDENT_WORDMARK: [&str; 8] = [
    r#"88888888888      d8b      888                   888"#,
    r#"    888          Y8P      888                   888"#,
    r#"    888                   888                   888"#,
    r#"    888  888d888 888  .d88888  .d88b.  88888b.  888888"#,
    r#"    888  888P"   888 d88" 888 d8P  Y8b 888 "88b 888"#,
    r#"    888  888     888 888  888 88888888 888  888 888"#,
    r#"    888  888     888 Y88b 888 Y8b.     888  888 Y88b."#,
    r#"    888  888     888  "Y88888  "Y8888  888  888  "Y888"#,
];
/// ASCII borders remain legible on Linux VTs and serial-like terminals.
const ASCII_BORDER: BorderSet = BorderSet {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Use the same screen headings in Ratatui and plain terminal output.
pub(super) fn title(screen: Screen) -> &'static str {
    match screen {
        Screen::Progress => "INSTALLING",
        Screen::Error => "INSTALLATION ERROR",
        Screen::Recovery => "RECOVERY",
        Screen::Success => "INSTALLATION COMPLETE",
        Screen::StreamUrl => "STREAM COSI FROM URL",
        Screen::HostConfigUrl => "INSTALL FROM HOST CONFIGURATION URL",
        Screen::Menu => "INSTALLER MENU",
    }
}

/// Gives panels a consistent ASCII border and VT-aware background.
fn block(title: &str, linux_vt: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(ASCII_BORDER)
        .style(Style::default().bg(theme::theme_color(COLOR_BACKGROUND, linux_vt)))
        .border_style(Style::default().fg(theme::theme_color(COLOR_FRAME, linux_vt)))
        .title(title)
}

/// Center against the widest row; the ASCII rows do not all have equal width.
fn wordmark_width() -> usize {
    TRIDENT_WORDMARK
        .iter()
        .fold(0, |width, row| width.max(row.len()))
}

/// Reserves wordmark space only when both the terminal width and height fit it.
fn header_height(viewport: Rect) -> u16 {
    if usize::from(viewport.width) >= wordmark_width()
        && viewport.height >= MIN_WORDMARK_TERMINAL_HEIGHT
    {
        WORDMARK_HEADER_HEIGHT
    } else {
        COMPACT_HEADER_HEIGHT
    }
}

/// Draws the wordmark when space permits, plus the installer status.
fn render_header(frame: &mut Frame, area: Rect, state: &str, state_color: Color, linux_vt: bool) {
    let brand = Style::default()
        .fg(theme::theme_color(COLOR_BRAND, linux_vt))
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    if area.height >= WORDMARK_HEADER_HEIGHT {
        lines.push(Line::default());
        let width = wordmark_width();
        lines.extend(
            TRIDENT_WORDMARK
                .iter()
                .map(|row| Line::from(Span::styled(format!("{row:<width$}"), brand))),
        );
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(APPLICATION_NAME, brand)));
    lines.push(Line::from(Span::styled(
        state,
        Style::default().fg(theme::theme_color(state_color, linux_vt)),
    )));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

/// Filters and scrolls retained logs, moving the latest result to the end on result screens.
fn log_panel(model: &Model, area: Rect) -> Paragraph<'_> {
    let (title, color) = match model.screen {
        Screen::Success => ("SUCCESS - Installation complete", COLOR_SUCCESS),
        Screen::Error => ("FAILURE - Installation stopped", COLOR_ERROR),
        _ => ("Activity / logs", COLOR_FRAME),
    };
    let mut lines = Vec::new();
    let header_lines = if matches!(model.screen, Screen::Progress | Screen::Menu) {
        2
    } else {
        0
    };
    if header_lines > 0 {
        lines.push(Line::from(Span::styled(
            model.state.activity.clone(),
            Style::default().fg(theme::theme_color(COLOR_BRAND, model.linux_vt)),
        )));
        lines.push(Line::default());
    }
    let mut logs = model
        .logs
        .iter()
        .filter(|entry| {
            entry.result.is_some() || diagnostics::log_visible(entry.level, model.verbosity)
        })
        .collect::<Vec<_>>();
    if matches!(model.screen, Screen::Success | Screen::Error) {
        if let Some(index) = logs.iter().rposition(|entry| entry.result.is_some()) {
            let result = logs.remove(index);
            logs.push(result);
        }
    }
    lines.extend(
        logs.into_iter()
            .flat_map(|entry| model::styled(entry, model.linux_vt).lines),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ASCII_BORDER)
        .style(Style::default().bg(theme::theme_color(COLOR_BACKGROUND, model.linux_vt)))
        .border_style(Style::default().fg(theme::theme_color(color, model.linux_vt)))
        .title(Span::styled(
            title,
            Style::default().fg(theme::theme_color(color, model.linux_vt)),
        ));
    let rows = log_window(
        &lines,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
        model.scroll,
    );
    // Paragraph only paints this prewrapped window; it never receives the full-history offset.
    Paragraph::new(rows).block(block)
}

/// Wraps before windowing so neither accumulated history nor one long line needs u16 scrolling.
/// `scroll` counts wrapped rows back from the end; overscroll returns the first viewport.
fn log_window<'a>(lines: &[Line<'a>], width: u16, height: u16, scroll: usize) -> Vec<Line<'a>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    // Greedy wrapping without inserted hyphens keeps each rendered row a slice of the source.
    let columns = usize::from(width);
    let options = Options::new(columns)
        .wrap_algorithm(WrapAlgorithm::FirstFit)
        .word_separator(WordSeparator::AsciiSpace)
        .word_splitter(WordSplitter::NoHyphenation);
    let row_counts = lines
        .iter()
        .map(|line| {
            if line.width() <= columns {
                1
            } else {
                textwrap::wrap(&line.to_string(), &options).len()
            }
        })
        .collect::<Vec<_>>();
    let visible_rows = usize::from(height);
    // Clamp overscroll at the top; all offsets stay wide until only the viewport remains.
    let mut skip = row_counts
        .iter()
        .sum::<usize>()
        .saturating_sub(visible_rows)
        .saturating_sub(scroll);
    let mut rows = Vec::with_capacity(visible_rows);
    for (line, count) in lines.iter().zip(row_counts) {
        if skip >= count {
            skip -= count;
            continue;
        }
        // Most historical lines fit already; copy only the handful inside the visible window.
        if line.width() <= columns {
            rows.push(line.clone());
            if rows.len() == visible_rows {
                return rows;
            }
            continue;
        }
        let text = line.to_string();
        let mut position = 0;
        for row in textwrap::wrap(&text, &options) {
            let start = position
                + text[position..]
                    .find(row.as_ref())
                    .expect("invariant: unhyphenated rows are source substrings");
            position = start + row.len();
            if skip > 0 {
                skip -= 1;
                continue;
            }
            rows.push(styled_slice(line, start..position));
            if rows.len() == visible_rows {
                return rows;
            }
        }
    }
    rows
}

/// Reapplies the original span colors to a wrapped row, whose byte boundaries come from textwrap.
fn styled_slice<'a>(line: &Line<'a>, range: Range<usize>) -> Line<'a> {
    let mut position = 0;
    let spans = line
        .spans
        .iter()
        .filter_map(|span| {
            let span_start = position;
            position += span.content.len();
            let start = range.start.max(span_start);
            let end = range.end.min(position);
            if start >= end {
                return None;
            }
            let range = start - span_start..end - span_start;
            // Borrow log-message slices; copy only spans already owned by the formatted prefix.
            let content = match &span.content {
                Cow::Borrowed(text) => Cow::Borrowed(&text[range]),
                Cow::Owned(text) => Cow::Owned(text[range].to_owned()),
            };
            Some(Span::styled(content, span.style))
        })
        .collect();
    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

/// Draws the current screen without changing its state or performing runtime actions.
/// Compact headers preserve room for full errors and every action on small terminals.
pub(super) fn render(frame: &mut Frame, model: &Model) {
    frame.render_widget(
        Block::default()
            .style(Style::default().bg(theme::theme_color(COLOR_BACKGROUND, model.linux_vt))),
        frame.area(),
    );
    let choices = model.choices();
    let action_height = if choices.is_empty() {
        0
    } else {
        u16::try_from(choices.len()).expect("invariant: static action lists fit in u16") + 2
    };
    let source_height = if model.screen == Screen::Recovery {
        0
    } else {
        2
    };
    let full_header = header_height(frame.area());
    // Keep diagnostics and every recovery action visible before reserving space for the wordmark.
    let header = if model.screen == Screen::Error
        || full_header + source_height + MIN_CONTENT_HEIGHT + action_height + FOOTER_HEIGHT
            > frame.area().height
    {
        COMPACT_HEADER_HEIGHT
    } else {
        full_header
    };
    let areas = Layout::vertical([
        Constraint::Length(header),
        Constraint::Length(source_height),
        Constraint::Min(MIN_CONTENT_HEIGHT),
        Constraint::Length(action_height),
        Constraint::Length(FOOTER_HEIGHT),
    ])
    .split(frame.area());
    render_header(
        frame,
        areas[0],
        title(model.screen),
        match model.screen {
            Screen::Error => COLOR_ERROR,
            Screen::Success => COLOR_SUCCESS,
            _ => COLOR_BRAND,
        },
        model.linux_vt,
    );
    if areas[1].width > 22 {
        let operation =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(21)]).split(areas[1]);
        frame.render_widget(
            Paragraph::new(diagnostics::display_text(&model.state.source))
                .wrap(Wrap { trim: false }),
            operation[0],
        );
        frame.render_widget(
            Paragraph::new(format!("Logs: {} [V]", model.verbosity))
                .alignment(Alignment::Right)
                .style(Style::default().fg(theme::theme_color(COLOR_BRAND, model.linux_vt))),
            operation[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(diagnostics::display_text(&model.state.source))
                .wrap(Wrap { trim: false }),
            areas[1],
        );
    }
    let content = match model.screen {
        _ if model.showing_logs() => log_panel(model, areas[2]),
        Screen::StreamUrl | Screen::HostConfigUrl => Paragraph::new(format!(
            "Enter an http:// or https:// URL:\n\n{}\n\nEnter: start   Esc: back",
            diagnostics::display_text(&model.url)
        ))
        .block(block("Remote source", model.linux_vt))
        .wrap(Wrap { trim: false }),
        _ => Paragraph::new(diagnostics::display_text(&model.state.details))
            .block(block("Result", model.linux_vt))
            .wrap(Wrap { trim: false }),
    };
    frame.render_widget(content, areas[2]);
    let choices = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            ListItem::new(format!(
                "{} {}",
                if index == model.selected { ">" } else { " " },
                choice.label()
            ))
            .style(if index == model.selected {
                Style::default()
                    .fg(theme::theme_color(COLOR_BRAND, model.linux_vt))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(choices).block(block("Actions", model.linux_vt)),
        areas[3],
    );
    let countdown = model
        .state
        .reboot_at
        .map(|deadline| {
            format!(
                "Automatic reboot in {}s; Shell holds reboot",
                deadline.saturating_duration_since(Instant::now()).as_secs()
            )
        })
        .unwrap_or_else(|| format!("Elapsed {}s", model.state.started.elapsed().as_secs()));
    let controls = if matches!(model.screen, Screen::StreamUrl | Screen::HostConfigUrl) {
        "Enter: start   Esc: return to recovery"
    } else {
        "Arrows: choose  Enter: select  S: shell  V: logs  PgUp/PgDn: scroll  Esc: back"
    };
    frame.render_widget(
        Paragraph::new(format!("{controls}\n{countdown}"))
            .style(Style::default().fg(theme::theme_color(COLOR_BRAND, model.linux_vt))),
        areas[4],
    );
    if model.verbosity_open {
        render_verbosity_picker(frame, model);
    }
}

/// Overlays the display-only log filter without modifying the stored diagnostics.
fn render_verbosity_picker(frame: &mut Frame, model: &Model) {
    let viewport = frame.area();
    let width = viewport.width.min(44);
    let height = viewport.height.min(10);
    let area = Rect::new(
        viewport.x + (viewport.width - width) / 2,
        viewport.y + (viewport.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let rows = VERBOSITY_LEVELS
        .iter()
        .enumerate()
        .map(|(index, level)| {
            Line::from(Span::styled(
                format!(
                    "{} {level}",
                    if index == model.verbosity_selected {
                        ">"
                    } else {
                        " "
                    }
                ),
                if index == model.verbosity_selected {
                    Style::default()
                        .fg(theme::theme_color(COLOR_BRAND, model.linux_vt))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ))
        })
        .chain([Line::default(), Line::from("Enter: apply   Esc: cancel")])
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(rows).block(block("Log verbosity / display only", model.linux_vt)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::slice;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use indoc::indoc;
    use log::LevelFilter;
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    use trident_client::{
        Completion as ClientCompletion, LogLevel, RebootDisposition, RemoteError,
        RemoteErrorCategory, ServicingEvent, ServicingKind,
    };

    use crate::{
        client::Event,
        diagnostics::{ResultKind, Source},
        APPLICATION_NAME,
    };

    use super::super::theme::{COLOR_DEBUG, COLOR_INFO, COLOR_TRACE};

    /// The full-size header and background keep their intended colors and spacing.
    #[test]
    fn wordmark_and_background_use_clear_neutrals_palette_at_80_by_24() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &Model::new())).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.content[0].bg, Color::Rgb(11, 17, 24));
        assert_eq!(buffer.content[14 * 80].fg, Color::Rgb(53, 69, 83));
        let padding = (80 - wordmark_width()) / 2;
        assert!(buffer.content[..80].iter().all(|cell| cell.symbol() == " "));
        for (row_index, expected) in TRIDENT_WORDMARK.iter().enumerate() {
            let row_index = row_index + 1;
            let row = buffer.content[row_index * 80..(row_index + 1) * 80]
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert_eq!(&row[padding..padding + expected.len()], *expected);
            for column in padding..padding + expected.len() {
                assert_eq!(
                    buffer.content[row_index * 80 + column].fg,
                    Color::Rgb(75, 216, 220)
                );
            }
            assert!(buffer.content[9 * 80..10 * 80]
                .iter()
                .all(|cell| cell.symbol() == " "));
        }
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains(APPLICATION_NAME), "{text}");
        assert!(text.contains("INSTALLING"), "{text}");
    }

    /// RPC log severities and the Trident source receive independent colors.
    #[test]
    fn rpc_logs_are_coloured_by_severity() {
        let mut model = Model::new();
        model.verbosity = LevelFilter::Trace;
        let records = [
            (LogLevel::Error, "error-line", Color::Rgb(255, 107, 115)),
            (LogLevel::Warn, "warn-line", Color::Rgb(245, 182, 93)),
            (LogLevel::Info, "info-line", Color::Rgb(229, 234, 236)),
            (LogLevel::Debug, "debug-line", Color::Rgb(167, 180, 189)),
            (LogLevel::Trace, "trace-line", Color::Rgb(99, 115, 129)),
        ];
        for (level, text, _) in records {
            model.log(Source::Trident, level, text.into());
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 32)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let buffer = terminal.backend().buffer();
        for (_, message, expected) in records {
            let row = buffer
                .content
                .chunks(80)
                .find(|row| {
                    row.iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                        .contains(message)
                })
                .unwrap();
            let text = row.iter().map(|cell| cell.symbol()).collect::<String>();
            assert_eq!(row[text.find(message).unwrap()].fg, expected);
            let source = text.find("TRIDENT").unwrap();
            assert_eq!(row[source].fg, Color::Rgb(114, 185, 165));
        }
    }

    /// VT palette slots distinguish log severities without changing RGB-mode colors.
    #[test]
    fn linux_vt_palette_uses_distinct_slots_for_a1_log_levels() {
        let entries = theme::vt_rgb_palette().unwrap();
        assert!(entries.contains(&(15, 229, 234, 236)));
        assert!(entries.contains(&(7, 167, 180, 189)));
        assert!(entries.contains(&(8, 99, 115, 129)));
        assert_eq!(theme::theme_color(COLOR_INFO, true), Color::White);
        assert_eq!(theme::theme_color(COLOR_DEBUG, true), Color::Gray);
        assert_eq!(theme::theme_color(COLOR_TRACE, true), Color::DarkGray);
        assert_eq!(theme::theme_color(COLOR_SUCCESS, true), Color::LightGreen);
        assert_eq!(
            theme::theme_color(COLOR_DEBUG, false),
            Color::Rgb(167, 180, 189)
        );

        let mut model = Model::new();
        model.linux_vt = true;
        model.verbosity = LevelFilter::Trace;
        for (level, message) in [
            (LogLevel::Info, "info-vt"),
            (LogLevel::Debug, "debug-vt"),
            (LogLevel::Trace, "trace-vt"),
        ] {
            model.log(Source::Trident, level, message.into());
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 32)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let buffer = terminal.backend().buffer();
        for (message, expected) in [
            ("info-vt", Color::White),
            ("debug-vt", Color::Gray),
            ("trace-vt", Color::DarkGray),
        ] {
            let row = buffer
                .content
                .chunks(80)
                .find(|row| {
                    row.iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                        .contains(message)
                })
                .unwrap();
            let text = row.iter().map(|cell| cell.symbol()).collect::<String>();
            assert_eq!(row[text.find(message).unwrap()].fg, expected);
        }
    }

    /// The source and verbosity indicator both fit on the standard-width status line.
    #[test]
    fn operation_and_verbosity_share_one_line() {
        let mut model = Model::new();
        model.state.source = "StreamDisk from file:///media/cosi/payload.cosi".into();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let row = terminal.backend().buffer().content[12 * 80..13 * 80]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(row.contains("StreamDisk from file://"), "{row}");
        assert!(row.contains("Logs: DEBUG [V]"), "{row}");
    }

    /// A success result remains highlighted even when earlier logs are filtered out.
    #[test]
    fn success_keeps_prior_logs_and_highlights_the_final_record() {
        let mut model = Model::new();
        model.log(
            Source::Trident,
            LogLevel::Debug,
            "Earlier disk action".into(),
        );
        let result = model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::NotRequired,
                servicing_kind: None,
                image_hash: None,
            },
        ))));
        assert_eq!(result.result, Some(ResultKind::Success));
        assert_eq!(
            result.plain(),
            "00:00 [INST:INFO] SUCCESS: Installation completed successfully.\n\
             00:00 [INST:INFO] Remove the installation media before booting the installed OS."
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let rows = terminal
            .backend()
            .buffer()
            .content
            .chunks(80)
            .collect::<Vec<_>>();
        let rendered = rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>();
        let title = rendered
            .iter()
            .position(|row| row.contains("SUCCESS - Installation complete"))
            .unwrap();
        assert_eq!(
            rows[title][rendered[title].find("SUCCESS").unwrap()].fg,
            Color::Rgb(93, 200, 148)
        );
        let result_row = rendered
            .iter()
            .position(|row| row.contains("SUCCESS: Installation"))
            .unwrap();
        assert_eq!(
            rows[result_row][rendered[result_row].find("SUCCESS:").unwrap()].fg,
            Color::Rgb(93, 200, 148)
        );
        model.key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), false);
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let earlier = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(earlier.contains("Earlier disk action"));
        model.scroll = 0;
        model.verbosity = LevelFilter::Off;
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("SUCCESS: Installation"));
        assert!(!text.contains("Earlier disk action"));
    }

    /// Wrapped diagnostics and activity cannot hide the latest result at 80x24.
    #[test]
    fn wrapped_log_rows_keep_the_result_visible_and_scrollable() {
        /// Five single-row presses move past the short result into earlier wrapped output.
        const SCROLL_ROWS: usize = 5;

        let mut model = Model::new();
        model.log(
            Source::Trident,
            LogLevel::Info,
            "Long preceding diagnostic ".repeat(40),
        );
        model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::NotRequired,
                servicing_kind: Some(ServicingKind::CleanInstall),
                image_hash: None,
            },
        ))));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let visible_text = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(
            visible_text(&terminal).contains("SUCCESS: Installation"),
            "{}",
            visible_text(&terminal)
        );
        for _ in 0..SCROLL_ROWS {
            model.key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), false);
        }
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(!visible_text(&terminal).contains("SUCCESS: Installation"));
        for _ in 0..SCROLL_ROWS {
            model.key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), false);
        }
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(visible_text(&terminal).contains("SUCCESS: Installation"));

        model.screen = Screen::Progress;
        model.state.activity = "Scanning media ".repeat(60);
        model.log(Source::Inst, LogLevel::Info, "Latest activity".into());
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(visible_text(&terminal).contains("Latest activity"));
    }

    /// Failures keep prior logs and the full diagnostic together in the same panel.
    #[test]
    fn failure_shows_logs_and_full_diagnostics_together() {
        let mut model = Model::new();
        model.log(Source::Trident, LogLevel::Warn, "Earlier warning".into());
        let result = model.event(Event::Error {
            details: "Network timeout\nUnderlying transport failure".into(),
            uncertain: false,
        });
        assert_eq!(result.result, Some(ResultKind::Failure));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("FAILURE - Installation stopped"));
        assert!(text.contains("Earlier warning"));
        assert!(text.contains("FAILURE: Network timeout"));
        for marker in ["FAILURE - Installation stopped", "FAILURE: Network timeout"] {
            let row = terminal
                .backend()
                .buffer()
                .content
                .chunks(80)
                .find(|row| {
                    row.iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                        .contains(marker)
                })
                .unwrap();
            let content = row.iter().map(|cell| cell.symbol()).collect::<String>();
            assert_eq!(
                row[content.find(marker).unwrap()].fg,
                Color::Rgb(255, 107, 115)
            );
        }
        assert!(text.contains("Underlying transport failure"));
        assert!(!text.contains("D: details"));
        model.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), false);
        assert_eq!(model.screen, Screen::Recovery);
    }

    /// A real-shaped error must remain readable without switching out of the log panel.
    #[test]
    fn error_chain_keeps_its_layout_in_the_scrollable_log_panel() {
        /// Covers the complete sample even when wrapped in the narrowest test terminal.
        const MAX_SCROLL_PRESSES: usize = 40;

        let details = indoc! {"
            Trident failed due to a servicing error

            Context:
                0: Failed to execute a clean install
                1: Failed to execute in chroot
                2: Step 'Configure' failed for subsystem 'network'
                3: Failed to write Netplan config
                4: Failed to open Netplan configuration file
                5: No such file or directory (os error 2)
        "};
        let mut model = Model::new();
        let record = model.event(Event::Response(ServicingEvent::Completed(Err(
            RemoteError {
                category: RemoteErrorCategory::Servicing,
                subkind: "write-netplan-config".into(),
                message: details.into(),
                error_message: "Failed to write Netplan config".into(),
                location: None,
            },
        ))));
        assert!(record
            .plain()
            .contains("[INST:ERROR]     5: No such file or directory"));
        assert!(model.state.details.contains(details));

        for (width, height) in [(40, 16), (80, 24), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let rows = |terminal: &Terminal<TestBackend>| {
                terminal
                    .backend()
                    .buffer()
                    .content
                    .chunks(usize::from(width))
                    .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                    .collect::<Vec<_>>()
            };
            model.scroll = 0;
            terminal.draw(|frame| render(frame, &model)).unwrap();
            if width == 160 {
                let text = rows(&terminal).join("\n");
                assert!(text.contains("FAILURE:"), "{text}");
                assert!(text.contains("Context:"), "{text}");
                assert!(
                    text.contains("    0: Failed to execute a clean install"),
                    "{text}"
                );
                assert!(text.contains("    5: No such file or directory"), "{text}");
                assert!(text.contains("Code: write-netplan-config"), "{text}");
            } else {
                let mut found = [false; 3];
                for _ in 0..MAX_SCROLL_PRESSES {
                    let text = rows(&terminal).join("\n");
                    for (seen, marker) in
                        found
                            .iter_mut()
                            .zip(["FAILURE:", "Context:", "No such file or directory"])
                    {
                        *seen |= text.contains(marker);
                    }
                    if found.iter().all(|seen| *seen) {
                        break;
                    }
                    model.key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), false);
                    terminal.draw(|frame| render(frame, &model)).unwrap();
                }
                assert!(
                    found.iter().all(|seen| *seen),
                    "{width}x{height}: error lines are not reachable: {found:?}"
                );
            }
        }
    }

    /// Many short multiline records must not push the latest failure beyond the viewport API.
    #[test]
    fn newest_failure_remains_reachable_beyond_u16_rows() {
        const HISTORY_RECORDS: usize = 499;
        const LINES_PER_RECORD: usize = 150;

        let mut model = Model::new();
        for index in 0..HISTORY_RECORDS {
            let first_line = if index == 0 { "oldest record\n" } else { "" };
            model.log(
                Source::Trident,
                LogLevel::Info,
                format!("{first_line}{}", "x\n".repeat(LINES_PER_RECORD)),
            );
        }
        model.fail(
            "latest failure\n    actionable underlying cause".into(),
            false,
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let text = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(text(&terminal).contains("FAILURE: latest failure"));
        assert!(text(&terminal).contains("actionable underlying cause"));

        model.scroll = usize::from(u16::MAX);
        model.key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), false);
        assert!(model.scroll > usize::from(u16::MAX));
        model.scroll = usize::MAX;
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(text(&terminal).contains("oldest record"));

        model.scroll = 0;
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(text(&terminal).contains("FAILURE: latest failure"));
    }

    /// One untruncated result can itself wrap past u16 rows; UTF-8 and span colors must survive.
    #[test]
    fn a_single_line_can_wrap_past_u16_rows() {
        let line = Line::from(vec![
            Span::styled("é", Style::default().fg(COLOR_ERROR)),
            Span::raw("x".repeat(usize::from(u16::MAX) * 2)),
            Span::styled("界", Style::default().fg(COLOR_SUCCESS)),
        ]);
        let latest = log_window(slice::from_ref(&line), 2, 1, 0);
        assert_eq!(latest[0].to_string(), "界");
        assert_eq!(latest[0].spans[0].style.fg, Some(COLOR_SUCCESS));

        let oldest = log_window(slice::from_ref(&line), 2, 1, usize::MAX);
        assert!(oldest[0].to_string().starts_with('é'));
        assert_eq!(oldest[0].spans[0].style.fg, Some(COLOR_ERROR));
    }

    /// Compact layouts retain the application heading and available actions.
    #[test]
    fn small_terminals_use_a_readable_compact_header() {
        assert_eq!(
            header_height(Rect::new(0, 0, 40, 24)),
            COMPACT_HEADER_HEIGHT
        );
        assert_eq!(
            header_height(Rect::new(0, 0, 80, 16)),
            COMPACT_HEADER_HEIGHT
        );
        let mut model = Model::new();
        model.screen = Screen::Success;
        let mut terminal = Terminal::new(TestBackend::new(40, 16)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains(APPLICATION_NAME), "{text}");
        assert!(text.contains("Reboot"), "{text}");
        assert!(text.contains("Shell"), "{text}");
    }

    /// Standard-size result and recovery screens expose their expected actions.
    #[test]
    fn screens_render_at_80_by_24_with_visible_actions() {
        for screen in [Screen::Recovery, Screen::Success, Screen::Error] {
            let mut model = Model::new();
            model.screen = screen;
            model.state.details = "Diagnostic or result details".into();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|frame| render(frame, &model)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("Shell"), "{text}");
            if screen == Screen::Success {
                assert!(text.contains("Reboot"), "{text}");
            }
            if screen == Screen::Recovery {
                assert!(text.contains("Retry"), "{text}");
                assert!(text.contains("Reboot"), "{text}");
                assert!(text.contains("Shutdown"), "{text}");
            }
        }
    }
}
