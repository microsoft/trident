//! Draws the Ratatui screens, their action lists and the scrollable log panel.

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::Set as BorderSet,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::APPLICATION_NAME;

use super::{
    model::{display_text, log_visible, Model, Screen},
    theme::{
        theme_color, COLOR_BACKGROUND, COLOR_BRAND, COLOR_ERROR, COLOR_FRAME, COLOR_SUCCESS,
        VERBOSITY_LEVELS,
    },
};

const WORDMARK_HEADER_HEIGHT: u16 = 12;
const COMPACT_HEADER_HEIGHT: u16 = 2;
const MIN_WORDMARK_TERMINAL_HEIGHT: u16 = 24;
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

/// Provides the short status heading for each installer screen.
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
        .style(Style::default().bg(theme_color(COLOR_BACKGROUND, linux_vt)))
        .border_style(Style::default().fg(theme_color(COLOR_FRAME, linux_vt)))
        .title(title)
}

/// Finds the widest wordmark row for layout and centering.
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
        .fg(theme_color(COLOR_BRAND, linux_vt))
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
        Style::default().fg(theme_color(state_color, linux_vt)),
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
            model.activity.clone(),
            Style::default().fg(theme_color(COLOR_BRAND, model.linux_vt)),
        )));
        lines.push(Line::default());
    }
    let mut logs = model
        .logs
        .iter()
        .filter(|entry| entry.result_color.is_some() || log_visible(entry.level, model.verbosity))
        .collect::<Vec<_>>();
    if matches!(model.screen, Screen::Success | Screen::Error) {
        if let Some(index) = logs.iter().rposition(|entry| entry.result_color.is_some()) {
            let result = logs.remove(index);
            logs.push(result);
        }
    }
    lines.extend(logs.into_iter().map(|entry| entry.styled(model.linux_vt)));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ASCII_BORDER)
        .style(Style::default().bg(theme_color(COLOR_BACKGROUND, model.linux_vt)))
        .border_style(Style::default().fg(theme_color(color, model.linux_vt)))
        .title(Span::styled(
            title,
            Style::default().fg(theme_color(color, model.linux_vt)),
        ));
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let visible_rows = usize::from(area.height.saturating_sub(2));
    let total_rows = paragraph.line_count(area.width.saturating_sub(2).max(1));
    let offset = total_rows
        .saturating_sub(visible_rows)
        .saturating_sub(usize::from(model.scroll));
    paragraph
        .block(block)
        .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0))
}

/// Draws the current screen, actions, log controls, and any pending reboot countdown.
pub(super) fn render(frame: &mut Frame, model: &Model) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme_color(COLOR_BACKGROUND, model.linux_vt))),
        frame.area(),
    );
    let choices = model.choices();
    let action_height = if choices.is_empty() {
        0
    } else {
        u16::try_from(choices.len())
            .expect("invariant: static action lists have at most four items")
            + 2
    };
    let areas = Layout::vertical([
        Constraint::Length(header_height(frame.area())),
        Constraint::Length(match model.screen {
            Screen::Recovery => 0,
            _ => 2,
        }),
        Constraint::Min(3),
        Constraint::Length(action_height),
        Constraint::Length(2),
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
            Paragraph::new(display_text(&model.source)).wrap(Wrap { trim: false }),
            operation[0],
        );
        frame.render_widget(
            Paragraph::new(format!("Logs: {} [V]", model.verbosity))
                .alignment(Alignment::Right)
                .style(Style::default().fg(theme_color(COLOR_BRAND, model.linux_vt))),
            operation[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(display_text(&model.source)).wrap(Wrap { trim: false }),
            areas[1],
        );
    }
    let content = match model.screen {
        _ if model.showing_logs() => log_panel(model, areas[2]),
        Screen::StreamUrl | Screen::HostConfigUrl => Paragraph::new(format!(
            "Enter an http:// or https:// URL:\n\n{}\n\nEnter: start   Esc: back",
            display_text(&model.url)
        ))
        .block(block("Remote source", model.linux_vt))
        .wrap(Wrap { trim: false }),
        Screen::Error => Paragraph::new(display_text(&model.details))
            .block(
                block("Full error details / Esc to return", model.linux_vt)
                    .border_style(Style::default().fg(theme_color(COLOR_ERROR, model.linux_vt))),
            )
            .wrap(Wrap { trim: false })
            .scroll((model.scroll, 0)),
        _ => Paragraph::new(display_text(&model.details))
            .block(block("Result", model.linux_vt))
            .wrap(Wrap { trim: false }),
    };
    frame.render_widget(content, areas[2]);
    let choices = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            ListItem::new(format!(
                "{} {choice}",
                if index == model.selected { ">" } else { " " }
            ))
            .style(if index == model.selected {
                Style::default()
                    .fg(theme_color(COLOR_BRAND, model.linux_vt))
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
        .reboot_at
        .map(|deadline| {
            format!(
                "Automatic reboot in {}s; Shell holds reboot",
                deadline.saturating_duration_since(Instant::now()).as_secs()
            )
        })
        .unwrap_or_else(|| format!("Elapsed {}s", model.started.elapsed().as_secs()));
    let controls = if matches!(model.screen, Screen::StreamUrl | Screen::HostConfigUrl) {
        "Enter: start   Esc: return to recovery".to_owned()
    } else {
        format!(
            "Arrows: choose  Enter: select  S: shell  V: logs{}  Esc: back",
            if model.screen == Screen::Error {
                "  D: details"
            } else {
                ""
            },
        )
    };
    frame.render_widget(
        Paragraph::new(format!("{controls}\n{countdown}"))
            .style(Style::default().fg(theme_color(COLOR_BRAND, model.linux_vt))),
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
                        .fg(theme_color(COLOR_BRAND, model.linux_vt))
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

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use log::LevelFilter;
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    use trident_client::{
        Completion as ClientCompletion, LogLevel, RebootDisposition, ServicingEvent, ServicingKind,
    };

    use crate::{client::Event, APPLICATION_NAME};

    use super::super::{
        model::LogSource,
        theme::{vt_rgb_palette, COLOR_DEBUG, COLOR_INFO, COLOR_TRACE},
    };

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
            model.log(LogSource::Trident, level, text.into());
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
        let entries = vt_rgb_palette().unwrap();
        assert!(entries.contains(&(15, 229, 234, 236)));
        assert!(entries.contains(&(7, 167, 180, 189)));
        assert!(entries.contains(&(8, 99, 115, 129)));
        assert_eq!(theme_color(COLOR_INFO, true), Color::White);
        assert_eq!(theme_color(COLOR_DEBUG, true), Color::Gray);
        assert_eq!(theme_color(COLOR_TRACE, true), Color::DarkGray);
        assert_eq!(theme_color(COLOR_SUCCESS, true), Color::LightGreen);
        assert_eq!(theme_color(COLOR_DEBUG, false), Color::Rgb(167, 180, 189));

        let mut model = Model::new();
        model.linux_vt = true;
        model.verbosity = LevelFilter::Trace;
        for (level, message) in [
            (LogLevel::Info, "info-vt"),
            (LogLevel::Debug, "debug-vt"),
            (LogLevel::Trace, "trace-vt"),
        ] {
            model.log(LogSource::Trident, level, message.into());
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
        model.source = "StreamDisk from file:///media/cosi/payload.cosi".into();
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
            LogSource::Trident,
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
        assert_eq!(result.result_color, Some(Color::Rgb(93, 200, 148)));
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
        let mut model = Model::new();
        model.log(
            LogSource::Trident,
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
        model.key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), false);
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(!visible_text(&terminal).contains("SUCCESS: Installation"));
        model.key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), false);
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(visible_text(&terminal).contains("SUCCESS: Installation"));

        model.screen = Screen::Progress;
        model.activity = "Scanning media ".repeat(60);
        model.log(LogSource::Inst, LogLevel::Info, "Latest activity".into());
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(visible_text(&terminal).contains("Latest activity"));
    }

    /// Failures show prior logs first while still allowing full diagnostic details.
    #[test]
    fn failure_shows_logs_by_default_and_keeps_full_details_accessible() {
        let mut model = Model::new();
        model.log(LogSource::Trident, LogLevel::Warn, "Earlier warning".into());
        let result = model.event(Event::Error {
            details: "Network timeout\nUnderlying transport failure".into(),
            uncertain: false,
        });
        assert_eq!(result.result_color, Some(Color::Rgb(255, 107, 115)));
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
        model.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE), false);
        assert!(!model.showing_logs());
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Underlying transport failure"));
        model.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), false);
        assert_eq!(model.screen, Screen::Error);
        assert!(model.showing_logs());
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
            model.details = "Diagnostic or result details".into();
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
                assert!(text.contains("Shutdown"), "{text}");
            }
        }
    }
}
