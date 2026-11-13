//! Runs the live installer console, coordinating operations, TTY ownership and power actions.

use std::{
    fs::OpenOptions,
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Error};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event as TerminalEvent},
    execute,
    terminal::{
        self, Clear as ClearTerminal, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use log::{error, Level};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::{
    signal::unix::{self, SignalKind},
    sync::mpsc,
    task::{self, JoinError, JoinHandle},
    time,
};

use osutils::{systemd, terminal as os_terminal};
use trident_client::{LogLevel, ServicingEvent};

use crate::{
    client::{self, Event},
    config::Config,
    source::{self, Request},
    INSTALLER_LOG_TX,
};

mod model;
mod output;
mod render;
mod theme;

use model::{display_text, log_visible, Action, LogSource, Model, Screen};
pub(super) use output::Output;
use render::{render, title};
use theme::{vt_rgb_palette, VERBOSITY_LEVELS};

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const INPUT_EVENTS_PER_FRAME: usize = 256;
const EVENT_CAPACITY: usize = 64;

struct Display {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: bool,
    linux_vt: bool,
}

impl Display {
    /// Creates a terminal display and enters its alternate screen.
    fn new(linux_vt: bool) -> Result<Self, Error> {
        let mut display = Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            active: false,
            linux_vt,
        };
        display.resume()?;
        Ok(display)
    }

    /// Restores raw-mode drawing and the VT palette after a shell exits.
    fn resume(&mut self) -> Result<(), Error> {
        terminal::enable_raw_mode()?;
        self.active = true;
        if self.linux_vt {
            os_terminal::set_vt_palette(&mut io::stdout(), &vt_rgb_palette()?)?;
        }
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        self.terminal.clear()?;
        Ok(())
    }

    /// Leaves raw mode and the alternate screen so an interactive shell can use the terminal.
    /// On Linux VTs, also clears the old UI and restores the original palette.
    fn suspend(&mut self) -> Result<(), Error> {
        let restore = (|| -> Result<(), Error> {
            terminal::disable_raw_mode()?;
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
            if self.linux_vt {
                // Linux VT leaves the old TUI visible when exiting its alternate screen.
                execute!(
                    self.terminal.backend_mut(),
                    ClearTerminal(ClearType::All),
                    MoveTo(0, 0)
                )?;
            }
            self.terminal.show_cursor()?;
            Ok(())
        })();
        let palette = if self.linux_vt {
            os_terminal::reset_vt_palette(&mut io::stdout())
        } else {
            Ok(())
        };
        self.active = false;
        restore?;
        palette?;
        Ok(())
    }
}

impl Drop for Display {
    /// Attempts terminal restoration if the display was not explicitly suspended.
    fn drop(&mut self) {
        if self.active {
            if let Err(error) = self.suspend() {
                error!("Failed to restore terminal: {error:#}");
            }
        }
    }
}

/// Reloads live configuration for each installation attempt.
/// Configuration read errors are reported as events rather than worker failures.
fn spawn_operation(
    config_path: PathBuf,
    request: Request,
    uncertain: bool,
    tx: mpsc::Sender<Event>,
) -> JoinHandle<Result<(), Error>> {
    task::spawn(async move {
        match Config::read(config_path) {
            Ok(settings) => client::execute(settings, request, uncertain, tx).await,
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
    })
}

/// Reports shell failures without changing the result of an independent installation.
fn shell_result_event(result: Result<Result<(), Error>, JoinError>) -> Option<Event> {
    match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(Event::InstallerLog {
            level: Level::Warn,
            message: format!("Recovery shell failed: {error:#}"),
        }),
        Err(error) => Some(Event::InstallerLog {
            level: Level::Warn,
            message: format!("Recovery shell task terminated: {error}"),
        }),
    }
}

/// Sanitizes received diagnostics before persistence, display, or serial mirroring.
fn redact_event(mut event: Event) -> Event {
    match &mut event {
        Event::InstallerLog { message, .. } | Event::Preparing(message) => {
            *message = source::redact_message_urls(message);
        }
        Event::Prepared { description, .. } => {
            *description = source::redact_message_urls(description);
        }
        Event::Error { details, .. } => {
            *details = source::redact_message_urls(details);
        }
        Event::Response(ServicingEvent::Log(record)) => {
            record.message = source::redact_message_urls(&record.message);
        }
        Event::Response(ServicingEvent::Completed(Err(error))) => {
            error.message = source::redact_message_urls(&error.message);
            error.error_message = source::redact_message_urls(&error.error_message);
        }
        _ => {}
    }
    event
}

/// Runs the installer event loop, starting autorun and offering recovery on a terminal.
/// Noninteractive runs return after work completes and fail if recovery needs user input;
/// `plain` selects text output instead of the full-screen display.
pub(super) async fn run(
    config_path: PathBuf,
    plain: bool,
    linux_vt: bool,
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
    model.linux_vt = linux_vt;
    let mut worker = Some(spawn_operation(
        config_path.clone(),
        Request::Autorun,
        false,
        tx.clone(),
    ));
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut display = if interactive && !plain {
        Some(Display::new(linux_vt)?)
    } else {
        None
    };
    let mut shell: Option<JoinHandle<Result<(), Error>>> = None;
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
            let event = redact_event(event);
            output.record(&event)?;
            let entry = model.event(event);
            output.announce(&entry, model.serial_verbosity);
            if display.is_none() && shell.is_none() && log_visible(entry.level, model.verbosity) {
                println!("{}", entry.plain());
            }
        }
        // A finished worker cannot enqueue a terminal event after the queue check.
        if worker.as_ref().is_some_and(JoinHandle::is_finished) && rx.is_empty() {
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
                if let Some(event) = shell_result_event(finished.await) {
                    let event = redact_event(event);
                    output.record(&event)?;
                    let warning = model.event(event);
                    output.announce(&warning, model.serial_verbosity);
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
                display.terminal.draw(|frame| render(frame, &model))?;
            } else if printed_screen != Some((model.screen, model.verbosity_open)) {
                if model.verbosity_open {
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
                printed_screen = Some((model.screen, model.verbosity_open));
            }
            for _ in 0..INPUT_EVENTS_PER_FRAME {
                if !event::poll(Duration::ZERO)? {
                    break;
                }
                let event = event::read()?;
                if let TerminalEvent::Key(key) = event {
                    if let Some(action) = model.key(key, worker.is_some()) {
                        match action {
                            Action::Shell => {
                                if let Some(display) = &mut display {
                                    display.suspend()?;
                                }
                                println!("\nShell: type exit to return. Installation continues; automatic reboot waits.");
                                let tty =
                                    OpenOptions::new().read(true).write(true).open("/dev/tty")?;
                                shell =
                                    Some(task::spawn_blocking(move || os_terminal::shell(&tty)));
                                break;
                            }
                            Action::Start(request) => {
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
                                    worker = Some(spawn_operation(
                                        config_path.clone(),
                                        request,
                                        model.uncertain,
                                        tx.clone(),
                                    ));
                                }
                            }
                            Action::BlockedPower => {
                                model.screen = if worker.is_some() {
                                    Screen::Menu
                                } else {
                                    Screen::Recovery
                                };
                                model.selected = 0;
                                model.activity = "Servicing may still be active; power actions are unavailable until Trident confirms it is idle".into();
                                let warning = model.log(
                                    LogSource::Inst,
                                    LogLevel::Warn,
                                    model.activity.clone(),
                                );
                                output.announce(&warning, model.serial_verbosity);
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
                model.screen = if worker.is_some() { Screen::Menu } else { Screen::Recovery };
                model.selected = 0;
            }
            _ = termination.recv() => return Err(anyhow!("Installer stopped; servicing may continue in Trident")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::{self, File};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    use trident_client::{
        Completion as ClientCompletion, LogRecord, RebootDisposition, RemoteError,
        RemoteErrorCategory, ServicingKind,
    };

    /// Creates an unmodified Enter press for navigation tests.
    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    /// Trident's COSI-loading messages must not expose signed URLs on any installer output.
    #[test]
    fn daemon_image_urls_are_redacted_before_recording_or_mirroring() {
        let root = TempDir::new().unwrap();
        let diagnostic = root.path().join("installer.log");
        let serial = root.path().join("serial.log");
        let mut output = Output {
            log: Some(File::create(&diagnostic).unwrap()),
            mirrors: vec![(serial.clone(), File::create(&serial).unwrap())],
        };
        let mut model = Model::new();
        for message in [
            "Attempting to load COSI file from 'https://user:password@example.test/image.cosi?sig=secret#fragment'",
            "Loaded COSI file from 'https://user:password@example.test/image.cosi?sig=secret#fragment' with hash 'abc'",
        ] {
            let event = redact_event(Event::Response(ServicingEvent::Log(LogRecord {
                timestamp: None,
                level: LogLevel::Info,
                message: message.into(),
                module: String::new(),
                target: String::new(),
                location: None,
            })));
            output.record(&event).unwrap();
            let entry = model.event(event);
            output.announce(&entry, model.serial_verbosity);
            assert!(entry.message.contains("https://example.test/image.cosi"));
            for secret in ["user", "password", "sig=secret", "fragment"] {
                assert!(!entry.message.contains(secret), "{entry:?}");
            }
        }
        for text in [
            fs::read_to_string(diagnostic).unwrap(),
            fs::read_to_string(serial).unwrap(),
        ] {
            assert!(text.contains("https://example.test/image.cosi"), "{text}");
            for secret in ["user", "password", "sig=secret", "fragment"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    /// Daemon failures cannot reintroduce a signed URL through structured error details.
    #[test]
    fn daemon_image_errors_redact_signed_urls() {
        let event = redact_event(Event::Response(ServicingEvent::Completed(Err(
            RemoteError {
                category: RemoteErrorCategory::Servicing,
                message: "Failed to load 'https://user:password@example.test/image.cosi?sig=secret#fragment'".into(),
                error_message: "HTTP failure at https://user:password@example.test/image.cosi?sig=secret#fragment".into(),
                subkind: "image-load-failed".into(),
                location: None,
            },
        ))));
        let mut model = Model::new();
        let entry = model.event(event);
        assert!(model.details.contains("https://example.test/image.cosi"));
        for secret in ["user", "password", "sig=secret", "fragment"] {
            assert!(!model.details.contains(secret), "{}", model.details);
            assert!(!entry.plain().contains(secret));
        }
    }

    /// A failed recovery shell cannot erase a successful servicing result or its reboot.
    #[test]
    fn shell_exit_failure_is_only_a_warning() {
        let mut model = Model::new();
        model.event(Event::Response(ServicingEvent::Completed(Ok(
            ClientCompletion {
                reboot: RebootDisposition::Required,
                servicing_kind: Some(ServicingKind::CleanInstall),
                image_hash: None,
            },
        ))));
        let reboot_at = model.reboot_at;
        assert!(reboot_at.is_some());
        assert!(shell_result_event(Ok(Ok(()))).is_none());
        let warning = model.event(shell_result_event(Ok(Err(anyhow!("exit 1")))).unwrap());
        assert_eq!(warning.level, LogLevel::Warn);
        assert!(warning.plain().contains("Recovery shell failed: exit 1"));
        assert_eq!(model.screen, Screen::Success);
        assert!(!model.failed);
        assert_eq!(model.reboot_at, reboot_at);
    }

    /// Recovery reports invalid reloaded configuration instead of using defaults.
    #[tokio::test]
    async fn recovery_reloads_configuration_without_using_defaults_on_error() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("installer.toml");
        for (text, diagnostic) in [("", "mode"), ("mode = 'not-a-mode'", "not-a-mode")] {
            fs::write(&path, text).unwrap();
            let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
            let worker = spawn_operation(path.clone(), Request::Autorun, false, tx);
            let Event::Error { details, uncertain } = rx.recv().await.unwrap() else {
                panic!("Invalid settings must fail before preparing a source");
            };
            assert!(!uncertain);
            assert!(details.contains(diagnostic), "{details}");
            worker.await.unwrap().unwrap();
            rx.try_recv().unwrap_err();
        }
    }

    /// An enqueued completion must be handled before the worker stops blocking power actions.
    #[tokio::test]
    async fn pending_completion_keeps_worker_active() {
        let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
        let worker = task::spawn(async move {
            tx.send(Event::Response(ServicingEvent::Completed(Ok(
                ClientCompletion {
                    reboot: RebootDisposition::NotRequired,
                    servicing_kind: Some(ServicingKind::CleanInstall),
                    image_hash: None,
                },
            ))))
            .await
            .unwrap();
            Ok::<(), Error>(())
        });
        time::timeout(Duration::from_secs(1), async {
            while !worker.is_finished() {
                task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let mut model = Model::new();
        model.screen = Screen::Recovery;
        model.selected = 3;
        assert!(!rx.is_empty());
        assert_eq!(
            model.key(enter(), !(rx.is_empty() && worker.is_finished())),
            Some(Action::BlockedPower)
        );
        model.event(rx.recv().await.unwrap());
        assert!(rx.is_empty());
        assert_eq!(model.screen, Screen::Success);
        worker.await.unwrap().unwrap();
    }

    /// An uncertain terminal event queued behind a full batch must block shutdown.
    #[tokio::test]
    async fn pending_disconnect_cannot_release_power_guard() {
        let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
        let worker = task::spawn(async move {
            for _ in 0..EVENT_CAPACITY {
                tx.send(Event::Preparing("Still writing".into()))
                    .await
                    .unwrap();
            }
            tx.send(Event::Error {
                details: "gRPC stream disconnected".into(),
                uncertain: true,
            })
            .await
            .unwrap();
            Ok::<(), Error>(())
        });
        for _ in 0..EVENT_CAPACITY {
            rx.recv().await.unwrap();
        }
        time::timeout(Duration::from_secs(1), async {
            while !worker.is_finished() {
                task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let mut model = Model::new();
        model.screen = Screen::Recovery;
        model.selected = 3;
        assert!(!rx.is_empty());
        assert_eq!(
            model.key(enter(), !(rx.is_empty() && worker.is_finished())),
            Some(Action::BlockedPower)
        );
        model.event(rx.recv().await.unwrap());
        assert!(model.uncertain);
        model.screen = Screen::Recovery;
        model.selected = 3;
        assert_eq!(model.key(enter(), false), Some(Action::BlockedPower));
        worker.await.unwrap().unwrap();
    }

    /// Rechecking an empty queue before worker completion misses a late disconnect.
    #[tokio::test]
    async fn disconnect_between_worker_and_queue_checks_stays_guarded() {
        let (tx, mut rx) = mpsc::channel(EVENT_CAPACITY);
        let (release, ready) = oneshot::channel::<()>();
        let worker = task::spawn(async move {
            ready.await.unwrap();
            tx.send(Event::Error {
                details: "gRPC stream disconnected".into(),
                uncertain: true,
            })
            .await
            .unwrap();
            Ok::<(), Error>(())
        });
        let previously_empty = rx.is_empty();
        release.send(()).unwrap();
        time::timeout(Duration::from_secs(1), async {
            while !worker.is_finished() {
                task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(previously_empty && worker.is_finished());
        assert!(!rx.is_empty());

        let mut model = Model::new();
        model.screen = Screen::Recovery;
        model.selected = 3;
        assert_eq!(
            model.key(enter(), !(worker.is_finished() && rx.is_empty())),
            Some(Action::BlockedPower)
        );
        model.event(rx.recv().await.unwrap());
        assert!(model.uncertain);
        model.screen = Screen::Recovery;
        model.selected = 3;
        assert_eq!(model.key(enter(), false), Some(Action::BlockedPower));
        worker.await.unwrap().unwrap();
    }
}
