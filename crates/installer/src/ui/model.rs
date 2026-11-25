//! Installer screen state, actions and bounded log history.

use std::{
    borrow::Cow,
    collections::VecDeque,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use log::{Level, LevelFilter};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

use trident_client::{LogLevel, ServicingEvent};

use crate::{
    client::{self, Completion, Event},
    config::SerialVerbosity,
    source::{self, Request},
};

use super::theme::{
    self, COLOR_DEBUG, COLOR_ERROR, COLOR_INFO, COLOR_INST, COLOR_SUCCESS, COLOR_TRACE,
    COLOR_TRIDENT, COLOR_WARN, VERBOSITY_LEVELS,
};

/// Give operators time to remove installer media before the automatic reboot.
const REBOOT_DELAY: Duration = Duration::from_secs(5);
/// Limit in-memory display history; the private diagnostic file keeps complete records.
const LOG_CAPACITY: usize = 500;
/// Bound ordinary display records; final results retain their full diagnostics.
const DISPLAY_LOG_CHARACTERS: usize = 4096;
/// Retain all severities, but start the display at Debug.
const DEFAULT_DISPLAY_VERBOSITY: LevelFilter = LevelFilter::Debug;
/// A single wrapped row avoids skipping diagnostics in short log panels.
const LOG_SCROLL_ROWS: usize = 1;
/// Elapsed prefixes show total minutes rather than wrapping after an hour.
const SECONDS_PER_MINUTE: u64 = 60;

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

/// Distinguishes local installer messages from records sent by Trident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogSource {
    Inst,
    Trident,
}

impl LogSource {
    /// Returns the source tag used in both displayed and mirrored diagnostics.
    fn label(self) -> &'static str {
        match self {
            Self::Inst => "INST",
            Self::Trident => "TRIDENT",
        }
    }

    /// Uses the source's theme color or its Linux VT equivalent.
    fn color(self, linux_vt: bool) -> Color {
        theme::theme_color(
            match self {
                Self::Inst => COLOR_INST,
                Self::Trident => COLOR_TRIDENT,
            },
            linux_vt,
        )
    }
}

/// One diagnostic, with a shortened copy retained in UI history.
#[derive(Debug, Clone)]
pub(super) struct LogEntry {
    /// Time since this installation attempt began.
    pub(super) elapsed: Duration,
    /// Controls the source tag and its color in both output modes.
    pub(super) source: LogSource,
    /// Used by display and serial filters independently.
    pub(super) level: LogLevel,
    /// Full message for output; the copy in `Model::logs` may be shortened.
    pub(super) message: String,
    /// Keeps a success or failure record visible even under a restrictive filter.
    pub(super) result_color: Option<Color>,
}

impl LogEntry {
    /// Formats elapsed time as a minutes-and-seconds prefix without wrapping at an hour.
    fn prefix(&self) -> String {
        let seconds = self.elapsed.as_secs();
        format!(
            "{:02}:{:02} [",
            seconds / SECONDS_PER_MINUTE,
            seconds % SECONDS_PER_MINUTE
        )
    }

    /// Sanitizes control characters and prefixes every line for plain and serial output.
    pub(super) fn plain(&self) -> String {
        let prefix = format!(
            "{}{}:{}] ",
            self.prefix(),
            self.source.label(),
            level_label(self.level)
        );
        display_text(&self.message)
            .split('\n')
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Preserves message lines and indentation, prefixing only the first line.
    /// `linux_vt` selects ANSI palette roles instead of RGB; result colors override severity.
    pub(super) fn styled(&self, linux_vt: bool) -> Text<'_> {
        let level = Style::default().fg(theme::theme_color(
            self.result_color.unwrap_or(log_color(self.level)),
            linux_vt,
        ));
        let surrounding = self
            .result_color
            .map(|color| Style::default().fg(theme::theme_color(color, linux_vt)))
            .unwrap_or_default();
        Text::from(
            self.message
                .split('\n')
                .enumerate()
                .map(|(index, message)| {
                    let mut spans = Vec::new();
                    if index == 0 {
                        spans.extend([
                            Span::styled(self.prefix(), surrounding),
                            Span::styled(
                                self.source.label(),
                                Style::default().fg(self.source.color(linux_vt)),
                            ),
                            Span::styled(":", surrounding),
                            Span::styled(
                                level_label(self.level),
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
}

/// Mutable UI state; events and keys change it without directly performing I/O.
#[derive(Debug)]
pub(super) struct Model {
    /// Current page, including recovery menus shown while work may still run.
    pub(super) screen: Screen,
    /// Sanitized description of the current Host Configuration or COSI source.
    pub(super) source: String,
    /// Last launched operation, retaining its original source for Retry.
    pub(super) last_request: Request,
    /// Short, most recent progress message above the log panel.
    pub(super) activity: String,
    /// Current result or failure, also used by plain output and headless errors.
    pub(super) details: String,
    /// Bounded diagnostic history, with final results kept in full.
    pub(super) logs: VecDeque<LogEntry>,
    /// On-screen filter; changing it does not discard stored records.
    pub(super) verbosity: LevelFilter,
    /// Separate serial filter loaded from installer settings.
    pub(super) serial_verbosity: LevelFilter,
    /// Whether the display-only log filter is being edited.
    pub(super) verbosity_open: bool,
    /// Highlighted option in the log filter picker.
    pub(super) verbosity_selected: usize,
    /// Highlighted action on the current screen.
    pub(super) selected: usize,
    /// Wrapped log rows to scroll back from the latest record.
    pub(super) scroll: usize,
    /// Recovery URL under edit; it is validated before starting a request.
    pub(super) url: String,
    /// The daemon may still be writing; block power actions until idleness is confirmed.
    pub(super) uncertain: bool,
    /// Media settings can disable the otherwise automatic reboot.
    reboot: bool,
    /// Countdown starts only after Trident reports completed work needing reboot.
    pub(super) reboot_at: Option<Instant>,
    /// Reset at each attempt so elapsed times refer to the current operation.
    pub(super) started: Instant,
    /// Headless runs return an error if recovery requires a terminal.
    pub(super) failed: bool,
    /// Selects ANSI palette roles instead of raw RGB for graphical Linux VTs.
    pub(super) linux_vt: bool,
}

impl Model {
    /// Starts on the progress screen with default display and serial log levels.
    pub(super) fn new() -> Self {
        Self {
            screen: Screen::Progress,
            source: String::new(),
            last_request: Request::Autorun,
            activity: "Preparing installer".into(),
            details: String::new(),
            logs: VecDeque::new(),
            verbosity: DEFAULT_DISPLAY_VERBOSITY,
            serial_verbosity: SerialVerbosity::Debug.filter(),
            verbosity_open: false,
            verbosity_selected: verbosity_index(DEFAULT_DISPLAY_VERBOSITY),
            selected: 0,
            scroll: 0,
            url: String::new(),
            uncertain: false,
            reboot: true,
            reboot_at: None,
            started: Instant::now(),
            failed: false,
            linux_vt: false,
        }
    }

    /// Logs the full failure, preserves earlier uncertainty and cancels automatic reboot.
    pub(super) fn fail(&mut self, details: String, uncertain: bool) -> LogEntry {
        self.details = details.clone();
        self.uncertain |= uncertain;
        self.reboot_at = None;
        self.failed = true;
        self.scroll = 0;
        self.selected = 0;
        self.screen = Screen::Error;
        self.result_log(LogLevel::Error, format!("FAILURE: {details}"), COLOR_ERROR)
    }

    /// Retains a bounded, sanitized display copy and returns the unshortened record.
    /// Display filtering does not affect retention; final results bypass this size limit.
    pub(super) fn log(&mut self, source: LogSource, level: LogLevel, message: String) -> LogEntry {
        let entry = LogEntry {
            elapsed: self.started.elapsed(),
            source,
            level,
            message: display_text(&message),
            result_color: None,
        };
        let mut characters = entry.message.chars();
        let mut displayed = characters
            .by_ref()
            .take(DISPLAY_LOG_CHARACTERS)
            .collect::<String>();
        if characters.next().is_some() {
            displayed.push_str(" [Display shortened; full record preserved in diagnostic log]");
        }
        if self.logs.len() == LOG_CAPACITY {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            message: displayed,
            ..entry.clone()
        });
        entry
    }

    /// Marks an installer result record with a color even when logs are filtered.
    fn result_log(&mut self, level: LogLevel, message: String, color: Color) -> LogEntry {
        let mut entry = self.log(LogSource::Inst, level, message);
        entry.result_color = Some(color);
        if let Some(last) = self.logs.back_mut() {
            // Results have no separate details view, so do not truncate their messages.
            *last = entry.clone();
        }
        entry
    }

    /// Applies one worker event and returns its diagnostic for plain and serial output.
    /// Only confirmed completion or idleness clears an uncertain servicing outcome.
    /// Unrecognized daemon events or completion kinds fail with an uncertain outcome.
    pub(super) fn event(&mut self, event: Event) -> LogEntry {
        match event {
            Event::InstallerLog { level, message } => self.log(
                LogSource::Inst,
                match level {
                    Level::Error => LogLevel::Error,
                    Level::Warn => LogLevel::Warn,
                    Level::Info => LogLevel::Info,
                    Level::Debug => LogLevel::Debug,
                    Level::Trace => LogLevel::Trace,
                },
                message,
            ),
            Event::Preparing(activity) => {
                self.activity = activity.clone();
                self.log(LogSource::Inst, LogLevel::Info, activity)
            }
            Event::Prepared {
                description,
                reboot,
                serial_verbosity,
            } => {
                self.source = description.clone();
                self.reboot = reboot;
                self.serial_verbosity = serial_verbosity.filter();
                self.log(LogSource::Inst, LogLevel::Info, description)
            }
            Event::IdleConfirmed => {
                self.uncertain = false;
                self.log(
                    LogSource::Inst,
                    LogLevel::Info,
                    "Trident confirmed that no servicing is active".into(),
                )
            }
            Event::Error { details, uncertain } => self.fail(details, uncertain),
            Event::Response(response) => match response {
                ServicingEvent::Started { .. } => {
                    self.activity = "Servicing started".into();
                    self.log(LogSource::Inst, LogLevel::Info, self.activity.clone())
                }
                ServicingEvent::Log(log) => {
                    let level = log.level;
                    if level == LogLevel::Info {
                        // Keep multiline dumps in the log panel, not in its one-line activity heading.
                        let heading = log
                            .message
                            .split_once('\n')
                            .map_or(log.message.as_str(), |(first, _)| first);
                        self.activity = display_text(heading);
                    }
                    self.log(LogSource::Trident, level, log.message)
                }
                ServicingEvent::Completed(completed) => match client::completion(completed) {
                    Ok(Completion::Success {
                        reboot_required,
                        performed,
                    }) => {
                        self.uncertain = false;
                        self.failed = false;
                        self.screen = Screen::Success;
                        self.selected = 0;
                        self.details = if performed {
                            "Installation completed successfully.\nRemove the installation media before booting the installed OS.".into()
                        } else {
                            "Trident reported that no servicing was performed.".into()
                        };
                        self.reboot_at = (self.reboot && reboot_required && performed)
                            .then(|| Instant::now() + REBOOT_DELAY);
                        self.result_log(
                            LogLevel::Info,
                            format!("SUCCESS: {}", self.details),
                            COLOR_SUCCESS,
                        )
                    }
                    Ok(Completion::Failure(details)) => {
                        self.uncertain = false;
                        self.fail(details, false)
                    }
                    Err(error) => {
                        let details = format!("{error:#}");
                        self.fail(details, true)
                    }
                },
                _ => self.fail(
                    "Unsupported Trident servicing event; installation outcome is unknown".into(),
                    true,
                ),
            },
        }
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
                if !self.uncertain {
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
    /// a running worker or uncertain outcome blocks power. Hidden or stale selections do nothing.
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
                    Err(error) => {
                        self.fail(format!("{error:#}"), false);
                        None
                    }
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
                    MenuItem::Retry => Some(Action::Start(self.last_request.clone())),
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
                return if (worker_active || self.uncertain)
                    && matches!(action, Some(Action::Reboot | Action::Poweroff))
                {
                    Some(Action::BlockedPower)
                } else {
                    action
                };
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

/// A request for the runtime to act; key handling itself never starts servicing.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Launch the selected recovery operation after releasing any previous worker.
    Start(Request),
    Shell,
    /// Reject power actions while a worker runs or the prior outcome is uncertain.
    BlockedPower,
    Reboot,
    Poweroff,
}

/// Removes terminal control characters while retaining newlines and expanding tabs.
pub(super) fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}

/// Applies the display filter, treating unknown daemon severities as warnings.
pub(super) fn log_visible(level: LogLevel, verbosity: LevelFilter) -> bool {
    let level = match level {
        LogLevel::Error => Level::Error,
        LogLevel::Warn | LogLevel::Unspecified | LogLevel::Unknown(_) => Level::Warn,
        LogLevel::Info => Level::Info,
        LogLevel::Debug => Level::Debug,
        LogLevel::Trace => Level::Trace,
        _ => Level::Warn,
    };
    level.to_level_filter() <= verbosity
}

/// Preserves the uppercase severity names used by TUI and serial records.
fn level_label(level: LogLevel) -> Cow<'static, str> {
    match level {
        LogLevel::Error => Cow::Borrowed("ERROR"),
        LogLevel::Warn => Cow::Borrowed("WARN"),
        LogLevel::Info => Cow::Borrowed("INFO"),
        LogLevel::Debug => Cow::Borrowed("DEBUG"),
        LogLevel::Trace => Cow::Borrowed("TRACE"),
        LogLevel::Unspecified => Cow::Borrowed("UNSPECIFIED"),
        LogLevel::Unknown(value) => Cow::Owned(format!("UNKNOWN({value})")),
        _ => Cow::Borrowed("UNSUPPORTED"),
    }
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

    use trident_client::{
        Completion as ClientCompletion, LogRecord, RebootDisposition, ServicingKind,
    };

    use crate::config::Config;

    /// Creates an unmodified Enter press for navigation tests.
    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    /// Changing display verbosity hides entries without deleting retained logs.
    #[test]
    fn live_verbosity_picker_filters_without_discarding_logs() {
        let mut model = Model::new();
        model.log(LogSource::Inst, LogLevel::Info, "ordinary event".into());
        model.log(
            LogSource::Trident,
            LogLevel::Trace,
            "diagnostic detail".into(),
        );
        assert_eq!(model.verbosity, LevelFilter::Debug);
        assert!(!log_visible(LogLevel::Trace, model.verbosity));
        model.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), false);
        assert!(model.verbosity_open);
        model.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), false);
        model.key(enter(), false);
        assert_eq!(model.verbosity, LevelFilter::Trace);
        assert!(!model.verbosity_open);
        assert!(log_visible(LogLevel::Trace, model.verbosity));
        assert_eq!(model.logs.len(), 2);
        model.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), false);
        model.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE), false);
        model.key(enter(), false);
        assert_eq!(model.verbosity, LevelFilter::Off);
        assert!(!log_visible(LogLevel::Error, model.verbosity));
        assert_eq!(model.logs.len(), 2);
    }

    /// Preparation events replace the initial idle activity with the current step.
    #[test]
    fn preparation_steps_replace_the_idle_status() {
        let mut model = Model::new();
        let step = "Looking for installer media by filesystem label";
        let entry = model.event(Event::Preparing(step.into()));
        assert_eq!(entry.source, LogSource::Inst);
        assert_eq!(entry.message, step);
        assert_eq!(model.activity, step);
    }

    /// Installer logger events retain their own source tag and severity styling.
    #[test]
    fn installer_logger_records_use_inst_source_and_colour() {
        let mut model = Model::new();
        let entry = model.event(Event::InstallerLog {
            level: Level::Debug,
            message: "Loaded installer settings".into(),
        });
        assert_eq!(entry.source, LogSource::Inst);
        assert_eq!(
            entry.plain(),
            "00:00 [INST:DEBUG] Loaded installer settings"
        );
        let styled = entry.styled(false);
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
        assert_eq!(entry.source, LogSource::Trident);
        assert_eq!(model.activity, "Preparing storage");
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
        assert!(log_visible(entry.level, LevelFilter::Warn));
        assert_eq!(log_color(entry.level), COLOR_WARN);
    }

    /// The autorun default schedules a reboot when servicing requires one.
    #[test]
    fn headless_autorun_keeps_automatic_reboot_enabled_by_default() {
        let config = Config::parse("mode = 'autorun'\nserialMode = 'logs'").unwrap();
        let mut model = Model::new();
        model.reboot = config.autorun.reboot;
        model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::Required,
                servicing_kind: None,
                image_hash: None,
            },
        ))));
        assert!(model.reboot_at.is_some());
    }

    /// Unknown servicing kinds fail rather than triggering an automatic reboot.
    #[test]
    fn unknown_completion_kind_does_not_claim_success() {
        let mut model = Model::new();
        model.reboot = true;
        let entry = model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::Required,
                servicing_kind: Some(ServicingKind::Unknown(42)),
                image_hash: None,
            },
        ))));
        assert!(model.failed);
        assert!(model.uncertain);
        assert!(model.reboot_at.is_none());
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
        assert!(model.failed);
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
        assert_eq!(model.details, details);
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
        assert_eq!(model.details, "Complete diagnostic information");
        model.screen = Screen::Success;
        model.select(MenuItem::Shell);
        assert_eq!(model.key(enter(), false), Some(Action::Shell));
        assert_eq!(model.screen, Screen::Success);
        model.select(MenuItem::Reboot);
        assert_eq!(model.key(enter(), false), Some(Action::Reboot));
    }

    /// Repeated interrupts and power choices cannot shut down an active installation.
    #[test]
    fn active_worker_cannot_reach_a_power_action() {
        let mut model = Model::new();
        let interrupt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        model.key(interrupt, true);
        assert_eq!(model.screen, Screen::Menu);
        model.key(interrupt, true);
        assert_eq!(model.screen, Screen::Menu);

        model.screen = Screen::Recovery;
        for (item, action) in [
            (MenuItem::Reboot, Action::Reboot),
            (MenuItem::Shutdown, Action::Poweroff),
        ] {
            model.select(item);
            assert_eq!(model.key(enter(), true), Some(Action::BlockedPower));
            assert_eq!(model.key(enter(), false), Some(action));
        }
        model.screen = Screen::Success;
        model.select(MenuItem::Reboot);
        assert_eq!(model.key(enter(), true), Some(Action::BlockedPower));
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
        assert!(!model.uncertain);
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
        assert!(!model.uncertain);
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
            model.last_request = request.clone();
            model.fail("Source temporarily unavailable".into(), true);
            model.key(enter(), false);
            assert_eq!(model.screen, Screen::Recovery);
            assert_eq!(model.key(enter(), false), Some(Action::Start(request)));
            assert!(model.uncertain);
        }
    }
}
