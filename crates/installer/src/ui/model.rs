//! Navigation, display filters and bounded display history; no servicing or power policy.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use log::LevelFilter;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use trident_client::LogLevel;

use crate::{
    diagnostics::{self, Record, ResultKind, Source},
    engine::{Command as Action, State},
    source::{self, Request},
};

use super::theme::{
    self, COLOR_DEBUG, COLOR_ERROR, COLOR_INFO, COLOR_INST, COLOR_SUCCESS, COLOR_TRACE,
    COLOR_TRIDENT, COLOR_WARN, VERBOSITY_LEVELS,
};

/// Limit in-memory display history; the private diagnostic file keeps complete records.
const LOG_CAPACITY: usize = 500;
/// Bound ordinary display records; final results retain their full diagnostics.
const DISPLAY_LOG_CHARACTERS: usize = 4096;
/// Retain all severities, but start the display at Debug.
const DEFAULT_DISPLAY_VERBOSITY: LevelFilter = LevelFilter::Debug;
/// A single wrapped row avoids skipping diagnostics in short log panels.
const LOG_SCROLL_ROWS: usize = 1;

/// Screen state used by both Ratatui and the plain-text fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Screen {
    Progress,
    Error,
    /// Actions after a failed attempt when no worker is active.
    Recovery,
    Success,
    StreamUrl,
    HostConfigUrl,
    /// Limited actions while an operation may still be writing.
    Menu,
}

/// A menu choice, independent of its row number or the runtime action it requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuItem {
    Retry,
    Shell,
    StreamUrl,
    HostConfigUrl,
    Reboot,
    Shutdown,
    ReturnToInstallation,
    ContinueToRecovery,
}

impl MenuItem {
    /// Text shared by the full-screen and plain menus; never used to dispatch actions.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Retry => "Retry",
            Self::Shell => "Shell",
            Self::StreamUrl => "Stream COSI from URL",
            Self::HostConfigUrl => "Install from Host Configuration URL",
            Self::Reboot => "Reboot",
            Self::Shutdown => "Shutdown",
            Self::ReturnToInstallation => "Return to installation",
            Self::ContinueToRecovery => "Continue to recovery",
        }
    }
}

/// Preserves message lines and indentation, prefixing only the first line.
/// `linux_vt` selects ANSI palette roles instead of RGB; result colors override severity.
pub(super) fn styled(record: &Record, linux_vt: bool) -> Text<'_> {
    let level = Style::default().fg(theme::theme_color(
        record
            .result
            .map(result_color)
            .unwrap_or(log_color(record.level)),
        linux_vt,
    ));
    let surrounding = record
        .result
        .map(result_color)
        .map(|color| Style::default().fg(theme::theme_color(color, linux_vt)))
        .unwrap_or_default();
    Text::from(
        record
            .message
            .split('\n')
            .enumerate()
            .map(|(index, message)| {
                let mut spans = Vec::new();
                if index == 0 {
                    spans.extend([
                        Span::styled(record.prefix(), surrounding),
                        Span::styled(
                            record.source.label(),
                            Style::default().fg(theme::theme_color(
                                match record.source {
                                    Source::Inst => COLOR_INST,
                                    Source::Trident => COLOR_TRIDENT,
                                },
                                linux_vt,
                            )),
                        ),
                        Span::styled(":", surrounding),
                        Span::styled(
                            diagnostics::level_label(record.level),
                            level.add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("] ", surrounding),
                    ]);
                }
                spans.push(Span::styled(message, level));
                Line::from(spans)
            })
            .collect::<Vec<_>>(),
    )
}

/// Maps domain result identity to a terminal-specific highlight.
fn result_color(result: ResultKind) -> Color {
    match result {
        ResultKind::Success => COLOR_SUCCESS,
        ResultKind::Failure => COLOR_ERROR,
    }
}

/// Presentation state and the last read-only engine snapshot used for drawing.
#[derive(Debug)]
pub(super) struct Model {
    /// Snapshot only; the engine owns the authoritative state and all mutations.
    pub(super) state: State,
    /// Current page, including menus opened while servicing continues.
    pub(super) screen: Screen,
    /// Bounded diagnostic history; final results are never shortened.
    pub(super) logs: VecDeque<Record>,
    /// Display filter, independent of serial and saved diagnostics.
    pub(super) verbosity: LevelFilter,
    /// Whether the display-only log filter is being edited.
    pub(super) verbosity_open: bool,
    /// Highlighted option in the verbosity picker.
    pub(super) verbosity_selected: usize,
    /// Highlighted action on the current screen.
    pub(super) selected: usize,
    /// Wrapped rows back from the latest visible log.
    pub(super) scroll: usize,
    /// URL under edit; it is validated before requesting a new attempt.
    pub(super) url: String,
    /// Graphical Linux VTs use programmed ANSI slots instead of RGB.
    pub(super) linux_vt: bool,
}

impl Model {
    /// Starts on Progress without starting servicing or changing machine state.
    pub(super) fn new() -> Self {
        Self {
            state: State::new(),
            screen: Screen::Progress,
            logs: VecDeque::new(),
            verbosity: DEFAULT_DISPLAY_VERBOSITY,
            verbosity_open: false,
            verbosity_selected: verbosity_index(DEFAULT_DISPLAY_VERBOSITY),
            selected: 0,
            scroll: 0,
            url: String::new(),
            linux_vt: false,
        }
    }

    /// Retains a display copy of an engine diagnostic, never changing servicing state.
    pub(super) fn record(&mut self, state: &State, mut record: Record) {
        self.state = state.clone();
        if let Some(result) = record.result {
            self.screen = match result {
                ResultKind::Success => Screen::Success,
                ResultKind::Failure => Screen::Error,
            };
            self.selected = 0;
            self.scroll = 0;
        } else {
            let mut chars = record.message.chars();
            let mut shortened = chars
                .by_ref()
                .take(DISPLAY_LOG_CHARACTERS)
                .collect::<String>();
            if chars.next().is_some() {
                shortened.push_str(" [Display shortened; full record preserved in diagnostic log]");
            }
            record.message = shortened;
        }
        if self.logs.len() == LOG_CAPACITY {
            self.logs.pop_front();
        }
        self.logs.push_back(record);
    }

    /// Resets navigation for an accepted attempt, preserving the retained log history.
    pub(super) fn begin_attempt(&mut self) {
        self.screen = Screen::Progress;
        self.scroll = 0;
        self.selected = 0;
    }

    /// Opens the limited running menu or the recovery page without affecting servicing.
    pub(super) fn interrupt(&mut self, active: bool) {
        self.screen = if active {
            Screen::Menu
        } else {
            Screen::Recovery
        };
        self.selected = 0;
    }

    /// Applies an event through the engine state when testing frontend presentation.
    #[cfg(test)]
    pub(super) fn event(&mut self, event: crate::client::Event) -> Record {
        let record = self.state.event(event);
        let state = self.state.clone();
        self.record(&state, record.clone());
        record
    }

    /// Creates a diagnostic through the engine state when testing frontend presentation.
    #[cfg(test)]
    pub(super) fn log(&mut self, source: Source, level: LogLevel, message: String) -> Record {
        let record = self.state.log(source, level, message);
        let state = self.state.clone();
        self.record(&state, record.clone());
        record
    }

    /// Exercises failure presentation without running a live attempt.
    #[cfg(test)]
    pub(super) fn fail(&mut self, details: String, uncertain: bool) -> Record {
        let record = self.state.fail(details, uncertain);
        let state = self.state.clone();
        self.record(&state, record.clone());
        record
    }

    /// Lists visible choices in keyboard order, excluding power actions while uncertain.
    /// The order controls navigation only; actions are dispatched by the item enum.
    pub(super) fn choices(&self) -> Vec<MenuItem> {
        match self.screen {
            Screen::Recovery => {
                let mut actions = vec![
                    MenuItem::Retry,
                    MenuItem::Shell,
                    MenuItem::StreamUrl,
                    MenuItem::HostConfigUrl,
                ];
                if !self.state.uncertain {
                    actions.extend([MenuItem::Reboot, MenuItem::Shutdown]);
                }
                actions
            }
            Screen::Success => vec![MenuItem::Reboot, MenuItem::Shell],
            Screen::Menu => vec![MenuItem::ReturnToInstallation, MenuItem::Shell],
            Screen::Error => vec![MenuItem::ContinueToRecovery, MenuItem::Shell],
            _ => Vec::new(),
        }
    }

    /// Shows servicing history on progress, menu and result screens.
    pub(super) fn showing_logs(&self) -> bool {
        matches!(
            self.screen,
            Screen::Progress | Screen::Menu | Screen::Success | Screen::Error
        )
    }

    /// Updates navigation for a key press and returns an action only when one is requested.
    /// URL entry and the verbosity picker consume keys before screen-level shortcuts;
    /// Hidden or stale selections do nothing; the engine authorizes every returned power command.
    pub(super) fn key(&mut self, key: KeyEvent, worker_active: bool) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        if self.verbosity_open {
            match key.code {
                KeyCode::Esc => self.verbosity_open = false,
                KeyCode::Up | KeyCode::BackTab => {
                    self.verbosity_selected = (self.verbosity_selected + VERBOSITY_LEVELS.len()
                        - 1)
                        % VERBOSITY_LEVELS.len();
                }
                KeyCode::Down | KeyCode::Tab => {
                    self.verbosity_selected =
                        (self.verbosity_selected + 1) % VERBOSITY_LEVELS.len();
                }
                KeyCode::Char(digit) if digit.is_ascii_digit() => {
                    if let Some(index) = digit.to_digit(10).and_then(|value| value.checked_sub(1)) {
                        if (index as usize) < VERBOSITY_LEVELS.len() {
                            self.verbosity_selected = index as usize;
                        }
                    }
                }
                KeyCode::Enter => {
                    self.verbosity = VERBOSITY_LEVELS[self.verbosity_selected];
                    self.verbosity_open = false;
                    self.scroll = 0;
                }
                _ => {}
            }
            return None;
        }
        if matches!(self.screen, Screen::StreamUrl | Screen::HostConfigUrl) {
            return match key.code {
                KeyCode::Esc => {
                    self.screen = Screen::Recovery;
                    None
                }
                KeyCode::Backspace => {
                    self.url.pop();
                    None
                }
                KeyCode::Char(c) if !c.is_control() => {
                    self.url.push(c);
                    None
                }
                KeyCode::Enter => match source::remote_url(self.url.trim()) {
                    Ok(url) => Some(Action::Start(if self.screen == Screen::StreamUrl {
                        Request::Stream(url)
                    } else {
                        Request::HostConfiguration(url.to_string())
                    })),
                    Err(error) => Some(Action::InvalidInput(format!("{error:#}"))),
                },
                _ => None,
            };
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.screen = if worker_active || self.screen == Screen::Progress {
                Screen::Menu
            } else {
                Screen::Recovery
            };
            self.selected = 0;
            return None;
        }
        let count = self.choices().len();
        match key.code {
            KeyCode::Char(digit) if digit.is_ascii_digit() => {
                if let Some(index) = digit.to_digit(10).and_then(|value| value.checked_sub(1)) {
                    if (index as usize) < count {
                        self.selected = index as usize;
                    }
                }
            }
            KeyCode::Char('s') => return Some(Action::Shell),
            KeyCode::Char('v' | 'V') => {
                self.verbosity_selected = verbosity_index(self.verbosity);
                self.verbosity_open = true;
                return None;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(LOG_SCROLL_ROWS);
                return None;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(LOG_SCROLL_ROWS);
                return None;
            }
            KeyCode::Esc => {
                self.screen = match self.screen {
                    Screen::Progress => Screen::Menu,
                    Screen::Menu => Screen::Progress,
                    Screen::Error => Screen::Recovery,
                    other => other,
                };
                self.selected = 0;
                return None;
            }
            _ => {}
        }
        if count == 0 {
            return None;
        }
        match key.code {
            KeyCode::Up | KeyCode::BackTab => self.selected = (self.selected + count - 1) % count,
            KeyCode::Down | KeyCode::Tab => self.selected = (self.selected + 1) % count,
            KeyCode::Enter => {
                let action = match self.choices().get(self.selected).copied()? {
                    MenuItem::Retry => Some(Action::Retry),
                    MenuItem::Shell => Some(Action::Shell),
                    MenuItem::StreamUrl => {
                        self.url.clear();
                        self.screen = Screen::StreamUrl;
                        None
                    }
                    MenuItem::HostConfigUrl => {
                        self.url.clear();
                        self.screen = Screen::HostConfigUrl;
                        None
                    }
                    MenuItem::Reboot => Some(Action::Reboot),
                    MenuItem::Shutdown => Some(Action::Poweroff),
                    MenuItem::ReturnToInstallation => {
                        self.screen = Screen::Progress;
                        None
                    }
                    MenuItem::ContinueToRecovery => {
                        self.screen = Screen::Recovery;
                        self.selected = 0;
                        None
                    }
                };
                return action;
            }
            _ => {}
        }
        None
    }

    /// Selects a visible item in tests, failing if the current screen does not offer it.
    #[cfg(test)]
    pub(super) fn select(&mut self, item: MenuItem) {
        self.selected = self
            .choices()
            .iter()
            .position(|choice| *choice == item)
            .expect("test menu item must be visible");
    }
}

/// Resolves picker position from the shared order; every LevelFilter must have an entry.
fn verbosity_index(level: LevelFilter) -> usize {
    VERBOSITY_LEVELS
        .iter()
        .position(|candidate| *candidate == level)
        .expect("invariant: all log levels are present in the verbosity picker")
}

/// Selects the severity color for Trident and installer records.
fn log_color(level: LogLevel) -> Color {
    match level {
        LogLevel::Error => COLOR_ERROR,
        LogLevel::Warn | LogLevel::Unspecified | LogLevel::Unknown(_) => COLOR_WARN,
        LogLevel::Info => COLOR_INFO,
        LogLevel::Debug => COLOR_DEBUG,
        LogLevel::Trace => COLOR_TRACE,
        _ => COLOR_WARN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::formatdoc;
    use log::Level;

    use trident_client::{
        Completion as ClientCompletion, LogRecord, RebootDisposition, ServicingEvent, ServicingKind,
    };

    use crate::{client::Event, config::Config};

    /// Creates an unmodified Enter press for navigation tests.
    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    /// Changing display verbosity hides entries without deleting retained logs.
    #[test]
    fn live_verbosity_picker_filters_without_discarding_logs() {
        let mut model = Model::new();
        model.log(Source::Inst, LogLevel::Info, "ordinary event".into());
        model.log(Source::Trident, LogLevel::Trace, "diagnostic detail".into());
        assert_eq!(model.verbosity, LevelFilter::Debug);
        assert!(!diagnostics::log_visible(LogLevel::Trace, model.verbosity));
        model.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), false);
        assert!(model.verbosity_open);
        model.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), false);
        model.key(enter(), false);
        assert_eq!(model.verbosity, LevelFilter::Trace);
        assert!(!model.verbosity_open);
        assert!(diagnostics::log_visible(LogLevel::Trace, model.verbosity));
        assert_eq!(model.logs.len(), 2);
        model.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), false);
        model.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE), false);
        model.key(enter(), false);
        assert_eq!(model.verbosity, LevelFilter::Off);
        assert!(!diagnostics::log_visible(LogLevel::Error, model.verbosity));
        assert_eq!(model.logs.len(), 2);
    }

    /// Preparation events replace the initial idle activity with the current step.
    #[test]
    fn preparation_steps_replace_the_idle_status() {
        let mut model = Model::new();
        let step = "Looking for installer media by filesystem label";
        let entry = model.event(Event::Preparing(step.into()));
        assert_eq!(entry.source, Source::Inst);
        assert_eq!(entry.message, step);
        assert_eq!(model.state.activity, step);
    }

    /// Installer logger events retain their own source tag and severity styling.
    #[test]
    fn installer_logger_records_use_inst_source_and_colour() {
        let mut model = Model::new();
        let entry = model.event(Event::InstallerLog {
            level: Level::Debug,
            message: "Loaded installer settings".into(),
        });
        assert_eq!(entry.source, Source::Inst);
        assert_eq!(
            entry.plain(),
            "00:00 [INST:DEBUG] Loaded installer settings"
        );
        let styled = styled(&entry, false);
        for (label, color) in [("INST", COLOR_INST), ("DEBUG", COLOR_DEBUG)] {
            let span = styled
                .lines
                .first()
                .unwrap()
                .spans
                .iter()
                .find(|span| span.content == label)
                .unwrap();
            assert_eq!(span.style.fg, Some(color));
        }
        assert_eq!(model.logs.len(), 1);
    }

    /// Domain logs retain Trident's source tag and update the displayed activity.
    #[test]
    fn trident_domain_log_event_updates_activity_and_serial_record() {
        let mut model = Model::new();
        let entry = model.event(Event::Response(ServicingEvent::Log(LogRecord {
            timestamp: None,
            level: LogLevel::Info,
            message: "Preparing storage".into(),
            module: String::new(),
            target: String::new(),
            location: None,
        })));
        assert_eq!(entry.source, Source::Trident);
        assert_eq!(model.state.activity, "Preparing storage");
        assert_eq!(entry.plain(), "00:00 [TRIDENT:INFO] Preparing storage");
    }

    /// Unknown daemon severities remain identifiable in serial output and visible as warnings.
    #[test]
    fn unknown_log_level_is_not_silently_discarded() {
        let mut model = Model::new();
        let entry = model.event(Event::Response(ServicingEvent::Log(LogRecord {
            timestamp: None,
            level: LogLevel::Unknown(42),
            message: "New severity".into(),
            module: String::new(),
            target: String::new(),
            location: None,
        })));
        assert_eq!(entry.plain(), "00:00 [TRIDENT:UNKNOWN(42)] New severity");
        assert!(diagnostics::log_visible(entry.level, LevelFilter::Warn));
        assert_eq!(log_color(entry.level), COLOR_WARN);
    }

    /// The autorun default schedules a reboot when servicing requires one.
    #[test]
    fn headless_autorun_keeps_automatic_reboot_enabled_by_default() {
        let config = Config::parse("mode = 'autorun'\nserialMode = 'logs'").unwrap();
        let mut model = Model::new();
        model.event(Event::Prepared {
            description: String::new(),
            reboot: config.autorun.reboot,
            serial_verbosity: config.serial_verbosity,
        });
        model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::Required,
                servicing_kind: None,
                image_hash: None,
            },
        ))));
        assert!(model.state.reboot_at.is_some());
    }

    /// Unknown servicing kinds fail rather than triggering an automatic reboot.
    #[test]
    fn unknown_completion_kind_does_not_claim_success() {
        let mut model = Model::new();
        let entry = model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::Required,
                servicing_kind: Some(ServicingKind::Unknown(42)),
                image_hash: None,
            },
        ))));
        assert!(model.state.failed);
        assert!(model.state.uncertain);
        assert!(model.state.reboot_at.is_none());
        assert!(entry.plain().contains("Unsupported daemon servicing kind"));
    }

    /// Missing inputs produce a tagged failure record suitable for serial output.
    #[test]
    fn errors_for_missing_inputs_are_tagged_for_serial_without_a_tui() {
        let mut model = Model::new();
        let entry = model.event(Event::Error {
            details: "No image or Host Configuration".into(),
            uncertain: false,
        });
        assert!(model.state.failed);
        assert_eq!(
            entry.plain(),
            "00:00 [INST:ERROR] FAILURE: No image or Host Configuration"
        );
    }

    /// Removing the details view must not make long failures lose their final cause.
    #[test]
    fn complete_failure_is_retained_beyond_the_normal_display_limit() {
        let context_line = "    Failed to configure a network interface\n";
        let context = context_line.repeat(DISPLAY_LOG_CHARACTERS / context_line.len() + 1);
        let details = formatdoc! {"
            Installation failed

            Context:
            {context}
                Failed to open Netplan configuration file
                No such file or directory (os error 2)
        "};
        let mut model = Model::new();
        let record = model.fail(details.clone(), false);
        assert_eq!(model.state.details, details);
        assert_eq!(model.logs.back().unwrap().message, record.message);
        assert!(record
            .message
            .contains("\n    No such file or directory (os error 2)\n"));
    }

    /// Continuing from an error opens recovery; shell actions leave their screen unchanged.
    #[test]
    fn error_continue_and_shell_keep_the_originating_screen() {
        let mut model = Model::new();
        model.fail("Complete diagnostic information".into(), false);
        assert!(model.key(enter(), false).is_none());
        assert_eq!(model.screen, Screen::Recovery);
        model.select(MenuItem::Shell);
        assert_eq!(model.key(enter(), false), Some(Action::Shell));
        assert_eq!(model.screen, Screen::Recovery);
        assert_eq!(model.state.details, "Complete diagnostic information");
        model.screen = Screen::Success;
        model.select(MenuItem::Shell);
        assert_eq!(model.key(enter(), false), Some(Action::Shell));
        assert_eq!(model.screen, Screen::Success);
        model.select(MenuItem::Reboot);
        assert_eq!(model.key(enter(), false), Some(Action::Reboot));
    }

    /// Repeated interrupts keep the running menu limited to navigation and shell access.
    #[test]
    fn active_worker_menu_does_not_offer_power_actions() {
        let mut model = Model::new();
        let interrupt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        model.key(interrupt, true);
        assert_eq!(model.screen, Screen::Menu);
        model.key(interrupt, true);
        assert_eq!(model.screen, Screen::Menu);

        assert!(!model.choices().contains(&MenuItem::Reboot));
        assert!(!model.choices().contains(&MenuItem::Shutdown));
    }

    /// Losing the client stream must not make power actions safe until Trident confirms idleness.
    #[test]
    fn uncertain_outcome_blocks_power_after_worker_exits() {
        let mut model = Model::new();
        model.event(Event::Error {
            details: "gRPC stream disconnected; servicing may continue".into(),
            uncertain: true,
        });
        model.screen = Screen::Recovery;
        assert!(!model.choices().contains(&MenuItem::Reboot));
        assert!(!model.choices().contains(&MenuItem::Shutdown));
        // A selection left beyond the shortened menu cannot dispatch a hidden power action.
        model.selected = model.choices().len();
        assert!(model.key(enter(), false).is_none());

        model.event(Event::IdleConfirmed);
        assert!(!model.state.uncertain);
        assert!(model.choices().contains(&MenuItem::Reboot));
        assert!(model.choices().contains(&MenuItem::Shutdown));
        for (item, action) in [
            (MenuItem::Reboot, Action::Reboot),
            (MenuItem::Shutdown, Action::Poweroff),
        ] {
            model.select(item);
            assert_eq!(model.key(enter(), false), Some(action));
        }

        model.event(Event::Error {
            details: "Another stream disconnected".into(),
            uncertain: true,
        });
        model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::NotRequired,
                servicing_kind: Some(ServicingKind::CleanInstall),
                image_hash: None,
            },
        ))));
        assert!(!model.state.uncertain);
        assert_eq!(model.key(enter(), false), Some(Action::Reboot));
    }

    /// Retry keeps the last source and carries uncertainty through to the next worker.
    #[test]
    fn recovery_retry_preserves_autorun_and_remote_sources() {
        let url = source::remote_url("https://example.test/image.cosi?sig=secret").unwrap();
        for request in [
            Request::Autorun,
            Request::Stream(url),
            Request::HostConfiguration("https://example.test/host.yaml?sig=secret".into()),
        ] {
            let mut model = Model::new();
            model.state.last_request = request.clone();
            model.fail("Source temporarily unavailable".into(), true);
            model.key(enter(), false);
            assert_eq!(model.screen, Screen::Recovery);
            assert_eq!(model.key(enter(), false), Some(Action::Retry));
            assert!(model.state.uncertain);
        }
    }
}
