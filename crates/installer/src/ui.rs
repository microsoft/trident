use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Error};
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use log::{error, warn, Level, LevelFilter};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::Set as BorderSet,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use tokio::{
    signal::unix::{self, SignalKind},
    sync::mpsc,
    task::{self, JoinHandle},
    time,
};
use url::Url;

use osutils::{systemd, terminal as os_terminal};
use trident_proto::v1::{servicing_response::Response as ResponseBody, LogLevel};

use crate::{
    client::{self, Completion, Event},
    config::{Config, SerialVerbosity},
    demo::{self, Scenario},
    source::{self, Request},
    APPLICATION_NAME, INSTALLER_LOG_TX,
};

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const REBOOT_DELAY: Duration = Duration::from_secs(5);
const EVENT_CAPACITY: usize = 64;
const LOG_CAPACITY: usize = 500;
const DISPLAY_LOG_CHARACTERS: usize = 4096;
const DEMO_SHELL_HISTORY: usize = 8;
const WORDMARK_HEADER_HEIGHT: u16 = 12;
const VERBOSITY_LEVELS: [LevelFilter; 6] = [
    LevelFilter::Off,
    LevelFilter::Error,
    LevelFilter::Warn,
    LevelFilter::Info,
    LevelFilter::Debug,
    LevelFilter::Trace,
];
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
const DEMO_SHELL_NOTICE: &str = "This shell is simulated. Commands are never executed.\nNo network, disk or power operations are performed.\nType exit or press Esc to return to the installer.";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Progress,
    Error,
    Recovery,
    Success,
    AlreadyPresent,
    Force,
    StreamUrl,
    HostConfigUrl,
    Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogSource {
    Inst,
    Trident,
}

impl LogSource {
    fn label(self) -> &'static str {
        match self {
            Self::Inst => "INST",
            Self::Trident => "TRIDENT",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Inst => Color::LightMagenta,
            Self::Trident => Color::LightGreen,
        }
    }
}

#[derive(Debug, Clone)]
struct LogEntry {
    elapsed: Duration,
    source: LogSource,
    level: LogLevel,
    message: String,
    result_color: Option<Color>,
}

impl LogEntry {
    fn prefix(&self) -> String {
        let seconds = self.elapsed.as_secs();
        format!("{:02}:{:02} [", seconds / 60, seconds % 60)
    }

    fn plain(&self) -> String {
        let prefix = format!("{}{}:{}] ", self.prefix(), self.source.label(), self.level);
        display_text(&self.message)
            .split('\n')
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn styled(&self) -> Line<'_> {
        let level = Style::default().fg(self.result_color.unwrap_or(log_color(self.level)));
        let surrounding = self
            .result_color
            .map(|color| Style::default().fg(color))
            .unwrap_or_default();
        Line::from(vec![
            Span::styled(self.prefix(), surrounding),
            Span::styled(
                self.source.label(),
                Style::default().fg(self.source.color()),
            ),
            Span::styled(":", surrounding),
            Span::styled(self.level.to_string(), level.add_modifier(Modifier::BOLD)),
            Span::styled("] ", surrounding),
            Span::styled(self.message.as_str(), level),
        ])
    }
}

#[derive(Debug)]
struct Model {
    screen: Screen,
    source: String,
    activity: String,
    details: String,
    logs: VecDeque<LogEntry>,
    verbosity: LevelFilter,
    serial_verbosity: LevelFilter,
    verbosity_open: bool,
    verbosity_selected: usize,
    selected: usize,
    scroll: u16,
    url: String,
    uncertain: bool,
    reboot: bool,
    reboot_at: Option<Instant>,
    stream_image: Option<Url>,
    started: Instant,
    failed: bool,
    error_details_open: bool,
}

impl Model {
    fn new() -> Self {
        Self {
            screen: Screen::Progress,
            source: String::new(),
            activity: "Preparing installer".into(),
            details: String::new(),
            logs: VecDeque::new(),
            verbosity: LevelFilter::Debug,
            serial_verbosity: SerialVerbosity::Debug.filter(),
            verbosity_open: false,
            verbosity_selected: 4,
            selected: 0,
            scroll: 0,
            url: String::new(),
            uncertain: false,
            reboot: true,
            reboot_at: None,
            stream_image: None,
            started: Instant::now(),
            failed: false,
            error_details_open: false,
        }
    }

    fn fail(&mut self, details: String, uncertain: bool) -> LogEntry {
        self.details = details.clone();
        self.uncertain |= uncertain;
        self.reboot_at = None;
        self.failed = true;
        self.scroll = 0;
        self.selected = 0;
        self.screen = Screen::Error;
        self.error_details_open = false;
        self.result_log(
            LogLevel::Error,
            format!("FAILURE: {details}"),
            Color::LightRed,
        )
    }

    fn log(&mut self, source: LogSource, level: LogLevel, message: String) -> LogEntry {
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
            .collect::<String>()
            .replace('\n', " ");
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

    fn result_log(&mut self, level: LogLevel, message: String, color: Color) -> LogEntry {
        let mut entry = self.log(LogSource::Inst, level, message);
        entry.result_color = Some(color);
        if let Some(last) = self.logs.back_mut() {
            last.result_color = Some(color);
        }
        entry
    }

    fn event(&mut self, event: Event) -> LogEntry {
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
                stream_image,
            } => {
                self.source = description.clone();
                self.reboot = reboot;
                self.serial_verbosity = serial_verbosity.filter();
                self.stream_image = stream_image;
                self.log(LogSource::Inst, LogLevel::Info, description)
            }
            Event::AlreadyPresent(details) => {
                self.details = details.clone();
                self.reboot_at = None;
                self.screen = Screen::AlreadyPresent;
                self.selected = 0;
                self.result_log(
                    LogLevel::Warn,
                    format!("ALREADY PRESENT: {details}"),
                    Color::Yellow,
                )
            }
            Event::Error { details, uncertain } => self.fail(details, uncertain),
            Event::Response(response) => match response.response {
                Some(ResponseBody::Started(_)) => {
                    self.activity = "Servicing started".into();
                    self.log(LogSource::Inst, LogLevel::Info, self.activity.clone())
                }
                Some(ResponseBody::Log(log)) => {
                    let level = log.level();
                    if level == LogLevel::Info {
                        self.activity = display_text(&log.message);
                    }
                    self.log(LogSource::Trident, level, log.message)
                }
                Some(ResponseBody::Completed(completed)) => match client::completion(completed) {
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
                            Color::LightGreen,
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
                None => {
                    let details = "Response has no body; installation outcome is unknown";
                    self.fail(details.into(), true)
                }
            },
        }
    }

    fn choices(&self) -> Vec<&'static str> {
        match self.screen {
            Screen::Recovery => vec![
                "Shell",
                "Stream COSI from URL",
                "Install from Host Configuration URL",
                "Shutdown",
            ],
            Screen::Success => vec!["Reboot", "Shell"],
            Screen::AlreadyPresent => vec!["Reboot", "Force reinstall", "Shell"],
            Screen::Force => vec!["Cancel", "Force reinstall and erase"],
            Screen::Menu => vec!["Return to installation", "Shell"],
            Screen::Error => vec!["Continue to recovery", "Shell"],
            _ => Vec::new(),
        }
    }

    fn showing_logs(&self) -> bool {
        matches!(
            self.screen,
            Screen::Progress | Screen::Menu | Screen::Success | Screen::AlreadyPresent
        ) || self.screen == Screen::Error && !self.error_details_open
    }

    fn key(&mut self, key: KeyEvent, demo: bool) -> Option<Action> {
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
                    Ok(url) => Some(Action::Start(
                        if self.screen == Screen::StreamUrl {
                            Request::Stream(url)
                        } else {
                            Request::HostConfiguration(url.to_string())
                        },
                        false,
                    )),
                    Err(error) => {
                        self.fail(format!("{error:#}"), false);
                        None
                    }
                },
                _ => None,
            };
        }
        if self.screen == Screen::Error && matches!(key.code, KeyCode::Char('d' | 'D')) {
            self.error_details_open = !self.error_details_open;
            self.scroll = 0;
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.screen = if self.screen == Screen::Progress {
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
            KeyCode::Char('q') if demo => return Some(Action::Quit),
            KeyCode::Char('s') => return Some(Action::Shell),
            KeyCode::Char('v' | 'V') => {
                self.verbosity_selected = VERBOSITY_LEVELS
                    .iter()
                    .position(|level| *level == self.verbosity)
                    .expect("invariant: all log levels are present in the verbosity picker");
                self.verbosity_open = true;
                return None;
            }
            KeyCode::PageUp => {
                self.scroll = if self.showing_logs() {
                    self.scroll.saturating_add(5)
                } else {
                    self.scroll.saturating_sub(5)
                };
                return None;
            }
            KeyCode::PageDown => {
                self.scroll = if self.showing_logs() {
                    self.scroll.saturating_sub(5)
                } else {
                    self.scroll.saturating_add(5)
                };
                return None;
            }
            KeyCode::Esc => {
                if self.screen == Screen::Error && self.error_details_open {
                    self.error_details_open = false;
                    self.scroll = 0;
                    return None;
                }
                self.screen = match self.screen {
                    Screen::Progress => Screen::Menu,
                    Screen::Menu => Screen::Progress,
                    Screen::Force => Screen::AlreadyPresent,
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
                return match (self.screen, self.selected) {
                    (Screen::Recovery, 0)
                    | (Screen::Success, 1)
                    | (Screen::AlreadyPresent, 2)
                    | (Screen::Menu, 1)
                    | (Screen::Error, 1) => Some(Action::Shell),
                    (Screen::Recovery, 1) => {
                        self.url.clear();
                        self.screen = Screen::StreamUrl;
                        None
                    }
                    (Screen::Recovery, 2) => {
                        self.url.clear();
                        self.screen = Screen::HostConfigUrl;
                        None
                    }
                    (Screen::Recovery, 3) => Some(Action::Poweroff),
                    (Screen::Success, 0) | (Screen::AlreadyPresent, 0) => Some(Action::Reboot),
                    (Screen::AlreadyPresent, 1) => {
                        self.screen = Screen::Force;
                        self.selected = 0;
                        None
                    }
                    (Screen::Force, 0) => {
                        self.screen = Screen::AlreadyPresent;
                        self.selected = 0;
                        None
                    }
                    (Screen::Force, 1) => Some(Action::Start(
                        self.stream_image
                            .clone()
                            .map(Request::Stream)
                            .unwrap_or(Request::Autorun),
                        true,
                    )),
                    (Screen::Menu, 0) => {
                        self.screen = Screen::Progress;
                        None
                    }
                    (Screen::Error, 0) => {
                        self.screen = Screen::Recovery;
                        self.selected = 0;
                        None
                    }
                    _ => None,
                }
            }
            _ => {}
        }
        None
    }
}

enum Action {
    Start(Request, bool),
    Shell,
    Reboot,
    Poweroff,
    Quit,
}

#[derive(Default)]
struct DemoShell {
    command: String,
    responses: VecDeque<String>,
}

impl DemoShell {
    fn key(&mut self, key: KeyEvent) -> bool {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.command.clear();
            return false;
        }
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Backspace => {
                self.command.pop();
            }
            KeyCode::Char(c) if !c.is_control() => self.command.push(c),
            KeyCode::Enter => {
                if self.command.trim() == "exit" {
                    return true;
                }
                if self.responses.len() == DEMO_SHELL_HISTORY {
                    self.responses.pop_front();
                }
                self.responses
                    .push_back("DEMO: Command not executed".into());
                self.command.clear();
            }
            _ => {}
        }
        false
    }

    fn text(&self) -> String {
        format!(
            "{DEMO_SHELL_NOTICE}\n\n{}\n\ndemo$ {}",
            self.responses
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            display_text(&self.command)
        )
    }
}

pub(super) struct Output {
    pub log: Option<File>,
    pub mirrors: Vec<(PathBuf, File)>,
}

impl Output {
    fn record(&mut self, event: &Event) -> Result<(), Error> {
        if let Some(log) = &mut self.log {
            writeln!(log, "{event:#?}").context("Failed to preserve installer diagnostics")?;
            log.flush()?;
        }
        Ok(())
    }

    fn announce(&mut self, entry: &LogEntry, verbosity: LevelFilter) {
        if !log_visible(entry.level, verbosity) {
            return;
        }
        self.mirrors.retain_mut(|(path, mirror)| {
            if let Err(error) = writeln!(mirror, "{}", entry.plain()) {
                warn!(
                    "Stopped mirroring installer status to '{}': {error}",
                    path.display()
                );
                return false;
            }
            true
        });
    }
}

struct Display {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: bool,
}

impl Display {
    fn new() -> Result<Self, Error> {
        let mut display = Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            active: false,
        };
        display.resume()?;
        Ok(display)
    }

    fn resume(&mut self) -> Result<(), Error> {
        terminal::enable_raw_mode()?;
        self.active = true;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        self.terminal.clear()?;
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), Error> {
        terminal::disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        self.active = false;
        Ok(())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        if self.active {
            if let Err(error) = self.suspend() {
                error!("Failed to restore terminal: {error:#}");
            }
        }
    }
}

fn spawn_operation(
    config_path: Option<PathBuf>,
    request: Request,
    force: bool,
    uncertain: bool,
    scenario: Option<Scenario>,
    tx: mpsc::Sender<Event>,
) -> JoinHandle<Result<(), Error>> {
    task::spawn(async move {
        match scenario {
            Some(scenario) => demo::run(if force { Scenario::Success } else { scenario }, tx).await,
            None => {
                let path = config_path
                    .context("Live installation requires an explicit configuration path")?;
                match Config::read(path) {
                    Ok(settings) => client::execute(settings, request, force, uncertain, tx).await,
                    Err(error) => {
                        client::send(
                            &tx,
                            Event::Error {
                                details: format!("{error:#}"),
                                uncertain,
                            },
                        )
                        .await
                    }
                }
            }
        }
    })
}

pub(super) async fn run(
    config_path: Option<PathBuf>,
    scenario: Option<Scenario>,
    plain: bool,
    mut output: Output,
) -> Result<(), Error> {
    let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
    let (log_tx, mut log_rx) = mpsc::unbounded_channel();
    INSTALLER_LOG_TX
        .set(log_tx)
        .map_err(|_| anyhow!("Installer log receiver already initialized"))?;
    let mut interrupts = unix::signal(SignalKind::interrupt())?;
    let mut termination = unix::signal(SignalKind::terminate())?;
    let mut model = Model::new();
    let mut worker = Some(spawn_operation(
        config_path.clone(),
        Request::Autorun,
        false,
        false,
        scenario,
        tx.clone(),
    ));
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut display = if interactive && !plain {
        Some(Display::new()?)
    } else {
        None
    };
    let mut shell: Option<JoinHandle<Result<(), Error>>> = None;
    let mut demo_shell: Option<DemoShell> = None;
    let mut printed_screen = None;
    loop {
        for _ in 0..EVENT_CAPACITY {
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(_) => match log_rx.try_recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            output.record(&event)?;
            let already_present = matches!(event, Event::AlreadyPresent(_));
            let entry = model.event(event);
            output.announce(&entry, model.serial_verbosity);
            if display.is_none()
                && shell.is_none()
                && demo_shell.is_none()
                && log_visible(entry.level, model.verbosity)
            {
                println!("{}", entry.plain());
            }
            if !interactive && already_present {
                model.failed = true;
                let explanation =
                    "Already installed; remove the media or confirm a force reinstall on a graphical console";
                model.details.push_str(&format!("\n{explanation}"));
                let error = model.log(LogSource::Inst, LogLevel::Error, explanation.into());
                output.announce(&error, model.serial_verbosity);
            }
        }
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = worker.take() {
                let failure = match finished.await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => {
                        Some(model.fail(format!("Installer worker failed: {error:#}"), true))
                    }
                    Err(error) => {
                        Some(model.fail(format!("Installer worker terminated: {error}"), true))
                    }
                };
                if let Some(failure) = failure {
                    output.announce(&failure, model.serial_verbosity);
                }
            }
        }
        if shell.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = shell.take() {
                let failure = match finished.await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(model.fail(format!("Shell failed: {error:#}"), false)),
                    Err(error) => {
                        Some(model.fail(format!("Shell task terminated: {error}"), false))
                    }
                };
                if let Some(failure) = failure {
                    output.announce(&failure, model.serial_verbosity);
                }
            }
            if let Some(display) = &mut display {
                display.resume()?;
            }
            printed_screen = None;
        }
        if shell.is_some() {
            tokio::select! {
                _ = time::sleep(FRAME_INTERVAL) => {}
                _ = termination.recv() => return Err(anyhow!("Installer stopped; servicing may continue in Trident")),
            }
            continue;
        }
        if model
            .reboot_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            model.reboot_at = None;
            if scenario.is_none() {
                let reboot = model.log(
                    LogSource::Inst,
                    LogLevel::Info,
                    "Rebooting after successful installation".into(),
                );
                output.announce(&reboot, model.serial_verbosity);
                match systemd::reboot() {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        let failure = model.fail(format!("Failed to reboot: {error:#}"), false);
                        output.announce(&failure, model.serial_verbosity);
                    }
                }
            }
        }
        if !interactive {
            if worker.is_none() && rx.is_empty() && log_rx.is_empty() && model.reboot_at.is_none() {
                if model.failed {
                    return Err(anyhow!(
                        "{}\nRecovery requires an interactive terminal.",
                        model.details
                    ));
                }
                return Ok(());
            }
        } else {
            if let Some(display) = &mut display {
                display.terminal.draw(|frame| match &demo_shell {
                    Some(shell) => render_demo_shell(frame, shell),
                    None => render(frame, &model, scenario.is_some()),
                })?;
            } else if printed_screen
                != Some((model.screen, demo_shell.is_some(), model.verbosity_open))
            {
                if let Some(shell) = &demo_shell {
                    println!("\nSIMULATED SHELL\n{}", shell.text());
                } else if model.verbosity_open {
                    println!("\nLOG VERBOSITY (display only)");
                    for (index, level) in VERBOSITY_LEVELS.iter().enumerate() {
                        println!("{}: {level}", index + 1);
                    }
                    println!("Number + Enter: apply; Esc: cancel");
                } else {
                    println!(
                        "\n{}\n{}",
                        title(model.screen),
                        display_text(&model.details)
                    );
                    for (index, choice) in model.choices().iter().enumerate() {
                        println!("{}: {choice}", index + 1);
                    }
                    if matches!(model.screen, Screen::StreamUrl | Screen::HostConfigUrl) {
                        println!("Enter an http:// or https:// URL, then press Enter");
                    }
                    println!("Number + Enter: select; S + Enter: shell; Esc: menu; V: verbosity");
                }
                printed_screen = Some((model.screen, demo_shell.is_some(), model.verbosity_open));
            }
            if event::poll(Duration::ZERO)? {
                let event = event::read()?;
                if let TerminalEvent::Key(key) = event {
                    if let Some(shell) = &mut demo_shell {
                        if shell.key(key) {
                            demo_shell = None;
                            printed_screen = None;
                        } else if key.code == KeyCode::Enter {
                            printed_screen = None;
                        }
                    } else if let Some(action) = model.key(key, scenario.is_some()) {
                        match action {
                            Action::Quit => return Ok(()),
                            Action::Shell if scenario.is_some() => {
                                demo_shell = Some(DemoShell::default());
                                printed_screen = None;
                            }
                            Action::Shell => {
                                if let Some(display) = &mut display {
                                    display.suspend()?;
                                }
                                println!("\nShell: type exit to return. Installation continues; automatic reboot waits.");
                                let tty =
                                    OpenOptions::new().read(true).write(true).open("/dev/tty")?;
                                shell =
                                    Some(task::spawn_blocking(move || os_terminal::shell(&tty)));
                            }
                            Action::Start(request, force) => {
                                if worker.is_some() {
                                    let failure = model.fail("An installer operation is still active. Wait for it to finish.".into(), true);
                                    output.announce(&failure, model.serial_verbosity);
                                } else {
                                    model.screen = Screen::Progress;
                                    model.activity = "Preparing installation".into();
                                    model.started = Instant::now();
                                    model.reboot_at = None;
                                    model.scroll = 0;
                                    model.selected = 0;
                                    let recovery_demo = scenario.map(|_| Scenario::Success);
                                    worker = Some(spawn_operation(
                                        config_path.clone(),
                                        request,
                                        force,
                                        model.uncertain,
                                        recovery_demo,
                                        tx.clone(),
                                    ));
                                }
                            }
                            Action::Reboot | Action::Poweroff if scenario.is_some() => {
                                model.reboot_at = None;
                                model.details = "DEMO: Machine power action suppressed".into();
                                model.screen = Screen::Success;
                                model.selected = 0;
                            }
                            Action::Reboot => {
                                let action = model.log(
                                    LogSource::Inst,
                                    LogLevel::Info,
                                    "Reboot requested".into(),
                                );
                                output.announce(&action, model.serial_verbosity);
                                if let Err(error) = systemd::reboot() {
                                    let failure =
                                        model.fail(format!("Failed to reboot: {error:#}"), false);
                                    output.announce(&failure, model.serial_verbosity);
                                } else {
                                    return Ok(());
                                }
                            }
                            Action::Poweroff => {
                                let action = model.log(
                                    LogSource::Inst,
                                    LogLevel::Info,
                                    "Shutdown requested".into(),
                                );
                                output.announce(&action, model.serial_verbosity);
                                if let Err(error) = systemd::poweroff() {
                                    let failure = model
                                        .fail(format!("Failed to shut down: {error:#}"), false);
                                    output.announce(&failure, model.serial_verbosity);
                                } else {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }
        tokio::select! {
            _ = time::sleep(FRAME_INTERVAL) => {}
            signal = interrupts.recv() => {
                if signal.is_none() {
                    return Err(anyhow!("Installer interrupt handler closed"));
                }
                if let Some(shell) = &mut demo_shell {
                    shell.command.clear();
                } else {
                    model.screen = if worker.is_some() { Screen::Menu } else { Screen::Recovery };
                    model.selected = 0;
                }
            }
            _ = termination.recv() => return Err(anyhow!("Installer stopped; servicing may continue in Trident")),
        }
    }
}

fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}

fn log_visible(level: LogLevel, verbosity: LevelFilter) -> bool {
    let level = match level {
        LogLevel::Error => Level::Error,
        LogLevel::Warn | LogLevel::Unspecified => Level::Warn,
        LogLevel::Info => Level::Info,
        LogLevel::Debug => Level::Debug,
        LogLevel::Trace => Level::Trace,
    };
    level.to_level_filter() <= verbosity
}

fn log_color(level: LogLevel) -> Color {
    match level {
        LogLevel::Error => Color::LightRed,
        LogLevel::Warn | LogLevel::Unspecified => Color::Rgb(255, 165, 0),
        LogLevel::Info => Color::LightBlue,
        LogLevel::Debug => Color::Rgb(170, 100, 255),
        LogLevel::Trace => Color::Gray,
    }
}

fn title(screen: Screen) -> &'static str {
    match screen {
        Screen::Progress => "INSTALLING",
        Screen::Error => "INSTALLATION ERROR",
        Screen::Recovery => "RECOVERY",
        Screen::Success => "INSTALLATION COMPLETE",
        Screen::AlreadyPresent => "IMAGE ALREADY PRESENT",
        Screen::Force => "CONFIRM FORCE REINSTALL",
        Screen::StreamUrl => "STREAM COSI FROM URL",
        Screen::HostConfigUrl => "INSTALL FROM HOST CONFIGURATION URL",
        Screen::Menu => "INSTALLER MENU",
    }
}

fn block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(ASCII_BORDER)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title)
}

fn wordmark_width() -> usize {
    TRIDENT_WORDMARK
        .iter()
        .fold(0, |width, row| width.max(row.len()))
}

fn header_height(viewport: Rect) -> u16 {
    if usize::from(viewport.width) >= wordmark_width()
        && viewport.height >= MIN_WORDMARK_TERMINAL_HEIGHT
    {
        WORDMARK_HEADER_HEIGHT
    } else {
        COMPACT_HEADER_HEIGHT
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &str, demo: bool, state_color: Color) {
    let brand = Style::default()
        .fg(Color::Cyan)
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
    let mut status = vec![Span::styled(state, Style::default().fg(state_color))];
    if demo {
        status.push(Span::styled(
            " / DEMO: ALL ACTIONS SIMULATED",
            Style::default().fg(Color::Cyan),
        ));
    }
    lines.push(Line::from(status));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn render_demo_shell(frame: &mut Frame, shell: &DemoShell) {
    let areas = Layout::vertical([
        Constraint::Length(header_height(frame.area())),
        Constraint::Min(4),
    ])
    .split(frame.area());
    render_header(frame, areas[0], "SIMULATED SHELL", true, Color::Cyan);
    frame.render_widget(
        Paragraph::new(shell.text())
            .block(block("No commands are executed"))
            .wrap(Wrap { trim: false }),
        areas[1],
    );
}

fn log_panel(model: &Model, area: Rect) -> Paragraph<'_> {
    let (title, color) = match model.screen {
        Screen::Success => ("SUCCESS - Installation complete", Color::LightGreen),
        Screen::Error => ("FAILURE - Installation stopped", Color::LightRed),
        Screen::AlreadyPresent => ("ALREADY PRESENT - No write", Color::Yellow),
        _ => ("Activity / logs", Color::DarkGray),
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
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::default());
    }
    let mut logs = model
        .logs
        .iter()
        .filter(|entry| entry.result_color.is_some() || log_visible(entry.level, model.verbosity))
        .collect::<Vec<_>>();
    if matches!(
        model.screen,
        Screen::Success | Screen::Error | Screen::AlreadyPresent
    ) {
        if let Some(index) = logs.iter().rposition(|entry| entry.result_color.is_some()) {
            let result = logs.remove(index);
            logs.push(result);
        }
    }
    let visible = usize::from(area.height.saturating_sub(2 + header_lines));
    let end = logs
        .len()
        .saturating_sub(usize::from(model.scroll).min(logs.len().saturating_sub(1)));
    let start = end.saturating_sub(visible);
    lines.extend(logs[start..end].iter().map(|entry| entry.styled()));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ASCII_BORDER)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title, Style::default().fg(color)));
    Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
}

fn render(frame: &mut Frame, model: &Model, demo: bool) {
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
            Screen::AlreadyPresent => 1,
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
        demo,
        match model.screen {
            Screen::Error => Color::LightRed,
            Screen::Success => Color::LightGreen,
            Screen::AlreadyPresent => Color::Yellow,
            _ => Color::Cyan,
        },
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
                .style(Style::default().fg(Color::Cyan)),
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
        Screen::StreamUrl | Screen::HostConfigUrl => Paragraph::new(format!("Enter an http:// or https:// URL:\n\n{}\n\nEnter: start   Esc: back", display_text(&model.url)))
            .block(block("Remote source")).wrap(Wrap { trim: false }),
        Screen::Force => Paragraph::new("Force reinstall will erase the disk selected by Trident.\n\nExisting partitions and files will be lost.\nCancel is selected by default.")
            .block(block("Destructive action")).wrap(Wrap { trim: false }),
        Screen::Error => Paragraph::new(display_text(&model.details))
            .block(block("Full error details / Esc to return")
                .border_style(Style::default().fg(Color::LightRed)))
            .wrap(Wrap { trim: false }).scroll((model.scroll, 0)),
        _ => Paragraph::new(display_text(&model.details))
            .block(block("Result")).wrap(Wrap { trim: false }),
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
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(choices).block(block("Actions")), areas[3]);
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
            "Arrows: choose  Enter: select  S: shell  V: logs{}  Esc: back{}",
            if model.screen == Screen::Error {
                "  D: details"
            } else {
                ""
            },
            if demo { "  Q: quit" } else { "" }
        )
    };
    frame.render_widget(
        Paragraph::new(format!("{controls}\n{countdown}")).style(Style::default().fg(Color::Cyan)),
        areas[4],
    );
    if model.verbosity_open {
        render_verbosity_picker(frame, model);
    }
}

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
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ))
        })
        .chain([Line::default(), Line::from("Enter: apply   Esc: cancel")])
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(rows).block(block("Log verbosity / display only")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use ratatui::backend::TestBackend;
    use tempfile::TempDir;
    use trident_proto::v1::{Completed, RebootStatus, ServicingResponse, StatusCode};

    #[test]
    fn unavailable_serial_mirror_does_not_stop_the_installer() {
        let root = TempDir::new().unwrap();
        let transcript = root.path().join("serial.log");
        let mut output = Output {
            log: None,
            mirrors: vec![
                (
                    PathBuf::from("/dev/full"),
                    OpenOptions::new().write(true).open("/dev/full").unwrap(),
                ),
                (
                    transcript.clone(),
                    OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .open(&transcript)
                        .unwrap(),
                ),
            ],
        };
        let entry = LogEntry {
            elapsed: Duration::from_secs(125),
            source: LogSource::Inst,
            level: LogLevel::Info,
            message: "Preparing installer".into(),
            result_color: None,
        };
        output.announce(&entry, LevelFilter::Debug);
        assert_eq!(output.mirrors.len(), 1);
        output.announce(
            &LogEntry {
                message: "Still running".into(),
                ..entry
            },
            LevelFilter::Debug,
        );
        assert_eq!(
            fs::read_to_string(transcript).unwrap(),
            "02:05 [INST:INFO] Preparing installer\n02:05 [INST:INFO] Still running\n"
        );
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    #[test]
    fn demo_shell_never_executes_commands_and_exit_returns() {
        let root = TempDir::new().unwrap();
        let marker = root.path().join("must-not-exist");
        let mut shell = DemoShell::default();
        for c in format!("touch {}", marker.display()).chars() {
            assert!(!shell.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert!(!shell.key(enter()));
        assert!(!marker.exists());
        assert!(shell.text().contains("DEMO: Command not executed"));
        for c in "exit".chars() {
            assert!(!shell.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert!(shell.key(enter()));
    }

    #[test]
    fn demo_shell_is_visibly_simulated() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render_demo_shell(frame, &DemoShell::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("SIMULATED SHELL"), "{text}");
        assert!(text.contains("Commands are never executed"), "{text}");
    }

    #[test]
    fn wordmark_is_cyan_and_preserved_at_80_by_24() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &Model::new(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
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
                assert_eq!(buffer.content[row_index * 80 + column].fg, Color::Cyan);
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
        assert!(text.contains("ALL ACTIONS SIMULATED"), "{text}");
    }

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
        model.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), true);
        assert!(model.verbosity_open);
        model.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), true);
        model.key(enter(), true);
        assert_eq!(model.verbosity, LevelFilter::Trace);
        assert!(!model.verbosity_open);
        assert!(log_visible(LogLevel::Trace, model.verbosity));
        assert_eq!(model.logs.len(), 2);
        model.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), true);
        model.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE), true);
        model.key(enter(), true);
        assert_eq!(model.verbosity, LevelFilter::Off);
        assert!(!log_visible(LogLevel::Error, model.verbosity));
        assert_eq!(model.logs.len(), 2);
    }

    #[test]
    fn preparation_steps_replace_the_idle_status() {
        let mut model = Model::new();
        let step = "Looking for installer media by filesystem label";
        let entry = model.event(Event::Preparing(step.into()));
        assert_eq!(entry.source, LogSource::Inst);
        assert_eq!(entry.message, step);
        assert_eq!(model.activity, step);
    }

    #[test]
    fn rpc_logs_are_coloured_by_severity() {
        let mut model = Model::new();
        model.verbosity = LevelFilter::Trace;
        let records = [
            (LogLevel::Error, "error-line", Color::LightRed),
            (LogLevel::Warn, "warn-line", Color::Rgb(255, 165, 0)),
            (LogLevel::Info, "info-line", Color::LightBlue),
            (LogLevel::Debug, "debug-line", Color::Rgb(170, 100, 255)),
            (LogLevel::Trace, "trace-line", Color::Gray),
        ];
        for (level, text, _) in records {
            model.log(LogSource::Trident, level, text.into());
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 32)).unwrap();
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
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
            assert_eq!(row[source].fg, Color::LightGreen);
        }
    }

    #[test]
    fn operation_and_verbosity_share_one_line() {
        let mut model = Model::new();
        model.source = "StreamDisk from file:///media/cosi/payload.cosi".into();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
        let row = terminal.backend().buffer().content[12 * 80..13 * 80]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(row.contains("StreamDisk from file://"), "{row}");
        assert!(row.contains("Logs: DEBUG [V]"), "{row}");
    }

    #[test]
    fn serial_verbosity_filters_independently_of_display() {
        let mut model = Model::new();
        let root = TempDir::new().unwrap();
        let serial = root.path().join("serial.log");
        let mut output = Output {
            log: None,
            mirrors: vec![(serial.clone(), File::create(&serial).unwrap())],
        };
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let entry = model.log(LogSource::Trident, level, "first\nsecond".into());
            output.announce(&entry, SerialVerbosity::Debug.filter());
        }
        let text = fs::read_to_string(&serial).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().all(|line| line.starts_with("00:00 [TRIDENT:")));
        assert_eq!(lines[0], "00:00 [TRIDENT:ERROR] first");
        assert_eq!(lines[1], "00:00 [TRIDENT:ERROR] second");
        assert!(!text.contains("\\n"));
        let trace = model.log(LogSource::Trident, LogLevel::Trace, "trace detail".into());
        output.announce(&trace, SerialVerbosity::Trace.filter());
        let text = fs::read_to_string(serial).unwrap();
        assert!(text
            .lines()
            .last()
            .unwrap()
            .ends_with("[TRIDENT:TRACE] trace detail"));
        assert!(!log_visible(LogLevel::Trace, model.verbosity));
        assert_eq!(model.logs.len(), 6);
    }

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
        assert_eq!(entry.styled().spans[1].style.fg, Some(Color::LightMagenta));
        assert_eq!(
            entry.styled().spans[3].style.fg,
            Some(Color::Rgb(170, 100, 255))
        );
        assert_eq!(model.logs.len(), 1);
    }

    #[test]
    fn headless_autorun_keeps_automatic_reboot_enabled_by_default() {
        let config = Config::parse("mode = 'autorun'\nserialMode = 'logs'").unwrap();
        let mut model = Model::new();
        model.reboot = config.autorun.reboot;
        model.event(Event::Response(ServicingResponse {
            response: Some(ResponseBody::Completed(Completed {
                status: StatusCode::Success.into(),
                reboot_status: RebootStatus::RebootRequired.into(),
                ..Default::default()
            })),
            ..Default::default()
        }));
        assert!(model.reboot_at.is_some());
    }

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

    #[test]
    fn success_keeps_prior_logs_and_highlights_the_final_record() {
        let mut model = Model::new();
        model.log(
            LogSource::Trident,
            LogLevel::Debug,
            "Earlier disk action".into(),
        );
        let result = model.event(Event::Response(ServicingResponse {
            response: Some(ResponseBody::Completed(Completed {
                status: StatusCode::Success.into(),
                reboot_status: RebootStatus::RebootNotRequired.into(),
                ..Default::default()
            })),
            ..Default::default()
        }));
        assert_eq!(result.result_color, Some(Color::LightGreen));
        assert_eq!(
            result.plain(),
            "00:00 [INST:INFO] SUCCESS: Installation completed successfully.\n\
             00:00 [INST:INFO] Remove the installation media before booting the installed OS."
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
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
            Color::LightGreen
        );
        let result_row = rendered
            .iter()
            .position(|row| row.contains("SUCCESS: Installation"))
            .unwrap();
        assert_eq!(
            rows[result_row][rendered[result_row].find("SUCCESS:").unwrap()].fg,
            Color::LightGreen
        );
        assert!(rendered
            .iter()
            .any(|row| row.contains("Earlier disk action")));
        model.verbosity = LevelFilter::Off;
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
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

    #[test]
    fn failure_shows_logs_by_default_and_keeps_full_details_accessible() {
        let mut model = Model::new();
        model.log(LogSource::Trident, LogLevel::Warn, "Earlier warning".into());
        let result = model.event(Event::Error {
            details: "Network timeout\nUnderlying transport failure".into(),
            uncertain: false,
        });
        assert_eq!(result.result_color, Some(Color::LightRed));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
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
        model.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE), true);
        assert!(model.error_details_open);
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Underlying transport failure"));
        model.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), true);
        assert_eq!(model.screen, Screen::Error);
        assert!(!model.error_details_open);
    }

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
        terminal.draw(|frame| render(frame, &model, true)).unwrap();
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

    #[tokio::test]
    async fn recovery_reloads_configuration_without_using_defaults_on_error() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("installer.toml");
        for (text, diagnostic) in [("", "mode"), ("mode = 'not-a-mode'", "not-a-mode")] {
            fs::write(&path, text).unwrap();
            let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
            let worker =
                spawn_operation(Some(path.clone()), Request::Autorun, false, false, None, tx);
            let Event::Error { details, uncertain } = rx.recv().await.unwrap() else {
                panic!("Invalid settings must fail before preparing a source");
            };
            assert!(!uncertain);
            assert!(details.contains(diagnostic), "{details}");
            worker.await.unwrap().unwrap();
            rx.try_recv().unwrap_err();
        }
    }

    #[test]
    fn error_continue_and_shell_keep_the_originating_screen() {
        let mut model = Model::new();
        model.fail("Complete diagnostic information".into(), false);
        assert!(model.key(enter(), false).is_none());
        assert_eq!(model.screen, Screen::Recovery);
        assert!(matches!(model.key(enter(), false), Some(Action::Shell)));
        assert_eq!(model.screen, Screen::Recovery);
        assert_eq!(model.details, "Complete diagnostic information");
        model.screen = Screen::Success;
        model.selected = 1;
        assert!(matches!(model.key(enter(), false), Some(Action::Shell)));
        assert_eq!(model.screen, Screen::Success);
        model.selected = 0;
        assert!(matches!(model.key(enter(), false), Some(Action::Reboot)));
    }

    #[test]
    fn force_is_never_the_default_and_requires_confirmation() {
        let mut model = Model::new();
        model.screen = Screen::AlreadyPresent;
        model.selected = 1;
        assert!(model.key(enter(), false).is_none());
        assert_eq!(model.screen, Screen::Force);
        assert_eq!(model.selected, 0);
        assert!(model.key(enter(), false).is_none());
        assert_eq!(model.screen, Screen::AlreadyPresent);
    }

    #[test]
    fn screens_render_at_80_by_24_with_visible_actions() {
        for screen in [
            Screen::Recovery,
            Screen::Success,
            Screen::Error,
            Screen::AlreadyPresent,
        ] {
            let mut model = Model::new();
            model.screen = screen;
            model.details = "Diagnostic or result details".into();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|frame| render(frame, &model, true)).unwrap();
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
