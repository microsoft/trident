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
use log::error;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::Set as BorderSet,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
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
    config::Config,
    demo::{self, Scenario},
    source::{self, Request},
    APPLICATION_NAME,
};

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const REBOOT_DELAY: Duration = Duration::from_secs(5);
const EVENT_CAPACITY: usize = 64;
const LOG_CAPACITY: usize = 500;
const DISPLAY_LOG_CHARACTERS: usize = 4096;
const DEMO_SHELL_HISTORY: usize = 8;
const WORDMARK_HEADER_HEIGHT: u16 = 10;
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

#[derive(Debug)]
struct Model {
    screen: Screen,
    source: String,
    activity: String,
    details: String,
    logs: VecDeque<(LogLevel, String)>,
    verbose: bool,
    selected: usize,
    scroll: u16,
    url: String,
    uncertain: bool,
    reboot: bool,
    reboot_at: Option<Instant>,
    stream_image: Option<Url>,
    started: Instant,
    failed: bool,
}

impl Model {
    fn new() -> Self {
        Self {
            screen: Screen::Progress,
            source: String::new(),
            activity: "Preparing installer".into(),
            details: String::new(),
            logs: VecDeque::new(),
            verbose: false,
            selected: 0,
            scroll: 0,
            url: String::new(),
            uncertain: false,
            reboot: true,
            reboot_at: None,
            stream_image: None,
            started: Instant::now(),
            failed: false,
        }
    }

    fn fail(&mut self, details: String, uncertain: bool) {
        self.details = details;
        self.uncertain |= uncertain;
        self.reboot_at = None;
        self.failed = true;
        self.scroll = 0;
        self.selected = 0;
        self.screen = Screen::Error;
    }

    fn event(&mut self, event: Event) -> Option<String> {
        match event {
            Event::Prepared {
                description,
                reboot,
                stream_image,
            } => {
                self.source = description;
                self.reboot = reboot;
                self.stream_image = stream_image;
                Some(self.source.clone())
            }
            Event::AlreadyPresent(details) => {
                self.details = details;
                self.reboot_at = None;
                self.screen = Screen::AlreadyPresent;
                self.selected = 0;
                Some(self.details.clone())
            }
            Event::Error { details, uncertain } => {
                self.fail(details, uncertain);
                Some(self.details.clone())
            }
            Event::Response(response) => match response.response {
                Some(ResponseBody::Started(_)) => {
                    self.activity = "Servicing started".into();
                    Some(self.activity.clone())
                }
                Some(ResponseBody::Log(log)) => {
                    let level = log.level();
                    let mut characters = log.message.chars();
                    let mut message = display_text(
                        &characters
                            .by_ref()
                            .take(DISPLAY_LOG_CHARACTERS)
                            .collect::<String>(),
                    );
                    if characters.next().is_some() {
                        message.push_str("\n[Display shortened; the full record is preserved in the diagnostic log]");
                    }
                    if level == LogLevel::Info {
                        self.activity = message.clone();
                    }
                    if self.logs.len() == LOG_CAPACITY {
                        self.logs.pop_front();
                    }
                    self.logs.push_back((level, message.clone()));
                    if matches!(
                        level,
                        LogLevel::Error | LogLevel::Warn | LogLevel::Info | LogLevel::Unspecified
                    ) {
                        Some(format!("{level}: {message}"))
                    } else {
                        None
                    }
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
                        Some(self.details.clone())
                    }
                    Ok(Completion::Failure(details)) => {
                        self.uncertain = false;
                        self.fail(details, false);
                        Some(self.details.clone())
                    }
                    Err(error) => {
                        self.fail(format!("{error:#}"), true);
                        Some(self.details.clone())
                    }
                },
                None => {
                    self.fail(
                        "Response has no body; installation outcome is unknown".into(),
                        true,
                    );
                    Some(self.details.clone())
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

    fn key(&mut self, key: KeyEvent, demo: bool) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
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
            KeyCode::Char('v') => {
                self.verbose = !self.verbose;
                return None;
            }
            KeyCode::PageUp => {
                self.scroll = if matches!(self.screen, Screen::Progress | Screen::Menu) {
                    self.scroll.saturating_add(5)
                } else {
                    self.scroll.saturating_sub(5)
                };
                return None;
            }
            KeyCode::PageDown => {
                self.scroll = if matches!(self.screen, Screen::Progress | Screen::Menu) {
                    self.scroll.saturating_sub(5)
                } else {
                    self.scroll.saturating_add(5)
                };
                return None;
            }
            KeyCode::Esc => {
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
    pub mirrors: Vec<File>,
    pub control: String,
}

impl Output {
    fn record(&mut self, event: &Event) -> Result<(), Error> {
        if let Some(log) = &mut self.log {
            writeln!(log, "{event:#?}").context("Failed to preserve installer diagnostics")?;
            log.flush()?;
        }
        Ok(())
    }

    fn announce(&mut self, message: &str) -> Result<(), Error> {
        for mirror in &mut self.mirrors {
            writeln!(
                mirror,
                "\r\n[installer; control {}] {}",
                self.control,
                display_text(message)
            )
            .context("Failed to mirror installer output to an active console")?;
        }
        Ok(())
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
            let Ok(event) = rx.try_recv() else { break };
            output.record(&event)?;
            if let Some(message) = model.event(event) {
                output.announce(&message)?;
                if display.is_none()
                    && shell.is_none()
                    && demo_shell.is_none()
                    && (interactive || !model.failed)
                {
                    println!("{}", display_text(&message));
                }
            }
        }
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = worker.take() {
                match finished.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        model.fail(format!("Installer worker failed: {error:#}"), true)
                    }
                    Err(error) => model.fail(format!("Installer worker terminated: {error}"), true),
                }
            }
        }
        if shell.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = shell.take() {
                match finished.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => model.fail(format!("Shell failed: {error:#}"), false),
                    Err(error) => model.fail(format!("Shell task terminated: {error}"), false),
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
                match systemd::reboot() {
                    Ok(()) => return Ok(()),
                    Err(error) => model.fail(format!("Failed to reboot: {error:#}"), false),
                }
            }
        }
        if !interactive {
            if worker.is_none() && rx.is_empty() {
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
            } else if printed_screen != Some((model.screen, demo_shell.is_some())) {
                if let Some(shell) = &demo_shell {
                    println!("\nSIMULATED SHELL\n{}", shell.text());
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
                    println!("Number + Enter: select; S + Enter: shell; Esc: menu; V: details");
                }
                printed_screen = Some((model.screen, demo_shell.is_some()));
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
                                    model.fail("An installer operation is still active. Wait for it to finish.".into(), true);
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
                            Action::Reboot => match systemd::reboot() {
                                Ok(()) => return Ok(()),
                                Err(error) => {
                                    model.fail(format!("Failed to reboot: {error:#}"), false)
                                }
                            },
                            Action::Poweroff => match systemd::poweroff() {
                                Ok(()) => return Ok(()),
                                Err(error) => {
                                    model.fail(format!("Failed to shut down: {error:#}"), false)
                                }
                            },
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
        let width = wordmark_width();
        lines.extend(
            TRIDENT_WORDMARK
                .iter()
                .map(|row| Line::from(Span::styled(format!("{row:<width$}"), brand))),
        );
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
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(action_height),
        Constraint::Length(2),
    ])
    .split(frame.area());
    render_header(
        frame,
        areas[0],
        title(model.screen),
        demo,
        if model.screen == Screen::Error {
            Color::LightRed
        } else {
            Color::Cyan
        },
    );
    frame.render_widget(
        Paragraph::new(display_text(&model.source)).wrap(Wrap { trim: false }),
        areas[1],
    );
    let content = match model.screen {
        Screen::Progress | Screen::Menu => {
            let mut lines = vec![Line::from(Span::styled(model.activity.clone(), Style::default().fg(Color::Cyan))), Line::default()];
            let logs = model.logs.iter().filter(|(level, _)| model.verbose || matches!(level, LogLevel::Error | LogLevel::Warn | LogLevel::Info | LogLevel::Unspecified)).collect::<Vec<_>>();
            let visible = usize::from(areas[2].height.saturating_sub(4));
            let end = logs.len().saturating_sub(usize::from(model.scroll).min(logs.len().saturating_sub(1)));
            let start = end.saturating_sub(visible);
            lines.extend(logs[start..end].iter().map(|(level, message)| Line::from(format!("{level}  {message}"))));
            Paragraph::new(lines).block(block("Activity / logs")).wrap(Wrap { trim: false })
        }
        Screen::StreamUrl | Screen::HostConfigUrl => Paragraph::new(format!("Enter an http:// or https:// URL:\n\n{}\n\nEnter: start   Esc: back", display_text(&model.url)))
            .block(block("Remote source")).wrap(Wrap { trim: false }),
        Screen::Force => Paragraph::new("Force reinstall will erase the disk selected by Trident.\n\nExisting partitions and files will be lost.\nCancel is selected by default.")
            .block(block("Destructive action")).wrap(Wrap { trim: false }),
        _ => Paragraph::new(display_text(&model.details))
            .block(if model.screen == Screen::Error {
                block("Error details / PgUp, PgDn to scroll").border_style(Style::default().fg(Color::LightRed))
            } else { block("Result") })
            .wrap(Wrap { trim: false }).scroll((model.scroll, 0)),
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
            "Up/Down + Enter: select   S: shell   V: details   Esc: menu{}",
            if demo { "   Q: quit" } else { "" }
        )
    };
    frame.render_widget(
        Paragraph::new(format!("{controls}\n{countdown}")).style(Style::default().fg(Color::Cyan)),
        areas[4],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use ratatui::backend::TestBackend;
    use tempfile::TempDir;

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
        for (row_index, expected) in TRIDENT_WORDMARK.iter().enumerate() {
            let row = buffer.content[row_index * 80..(row_index + 1) * 80]
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert_eq!(&row[padding..padding + expected.len()], *expected);
            for column in padding..padding + expected.len() {
                assert_eq!(buffer.content[row_index * 80 + column].fg, Color::Cyan);
            }
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
