//! Installer orchestration and safety policy, independent of terminal libraries.

use std::{
    fs::OpenOptions,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Error};
use log::Level;
use tokio::{
    signal::unix::{self, SignalKind},
    sync::mpsc,
    task::{self, JoinError, JoinHandle},
    time,
};

use osutils::systemd;

use crate::{
    client::{AttemptOutcome, Event},
    diagnostics::{self, Output, Record},
    source::Request,
    terminal, INSTALLER_LOG_TX,
};

mod attempt;
mod state;

pub(crate) use state::State;

/// Bound event processing so progress storms cannot starve commands or shutdown signals.
const EVENT_CAPACITY: usize = 64;
/// Poll servicing and frontend input without busy-spinning.
const TICK_INTERVAL: Duration = Duration::from_millis(100);

/// Requested application action; only the engine may authorize servicing or power operations.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Reload settings and run this source, after the previous worker has completely drained.
    Start(Request),
    /// Repeat the last launched request with fresh settings and the existing uncertainty guard.
    Retry,
    /// Servicing continues, but automatic reboot waits until the recovery shell exits.
    Shell,
    /// UI validation failed before a request could be launched.
    InvalidInput(String),
    /// Ask the engine to reboot only if its authoritative servicing state permits it.
    Reboot,
    /// Ask the engine to shut down only if its authoritative servicing state permits it.
    Poweroff,
}

/// Presentation notification; none grants permission to run an operation.
pub(crate) enum Update {
    /// Complete diagnostic; a frontend can keep its own bounded display history.
    Record(Record),
    /// The engine accepted a new attempt.
    AttemptStarted,
    /// Open controls appropriate for the actual worker state.
    Interrupted { active: bool },
}

/// Minimal presentation boundary shared by graphical and line-oriented frontends.
/// Implementations must not execute servicing or power commands themselves.
pub(crate) trait Frontend {
    /// Whether an operator can provide recovery input.
    fn interactive(&self) -> bool;
    /// Observe state and diagnostics; changing display filters must not mutate engine state.
    fn update(&mut self, state: &State, update: Update);
    /// Refresh output without changing servicing state.
    fn draw(&mut self, state: &State) -> Result<(), Error>;
    /// Return one requested command or None; must not block waiting for input.
    fn poll(&mut self, state: &State, active: bool) -> Result<Option<Command>, Error>;
    /// Give an external shell ownership of the display; engine event processing continues.
    fn suspend(&mut self) -> Result<(), Error>;
    /// Restore display ownership after the shell, without changing its installation result.
    fn resume(&mut self) -> Result<(), Error>;
}

/// Owns the sole worker and its queue; queued terminal events keep the worker logically active.
struct Session {
    /// Authoritative result, retry source and reboot policy, never owned by presentation.
    state: State,
    /// Reload the same bootstrap path before each accepted attempt.
    config_path: PathBuf,
    /// Retained until both task completion and its terminal queue are observed.
    worker: Option<JoinHandle<Result<AttemptOutcome, Error>>>,
    /// Each attempt receives a sender, not access to the controller or frontend.
    tx: mpsc::Sender<Event>,
    /// Drained before permitting another write or releasing the power guard.
    rx: mpsc::Receiver<Event>,
}

impl Session {
    /// Initializes a session without touching disks or starting a worker.
    fn new(config_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
        Self {
            state: State::new(),
            config_path,
            worker: None,
            tx,
            rx,
        }
    }

    /// A finished task remains active until its final queued result has been applied.
    fn active(&self) -> bool {
        self.worker.is_some()
    }

    /// Rejects overlapping attempts; uncertainty is passed to the client for idle confirmation.
    fn start(&mut self, request: Request) -> Result<(), Error> {
        if self.active() {
            bail!("An installer operation is still active. Wait for it to finish.");
        }
        self.state.begin(request.clone());
        self.worker = Some(attempt::spawn(
            self.config_path.clone(),
            request,
            self.state.uncertain,
            self.tx.clone(),
        ));
        Ok(())
    }

    /// Redact before every sink; persistence always receives the complete event.
    fn publish(
        &mut self,
        event: Event,
        frontend: &mut impl Frontend,
        output: &mut Output,
    ) -> Result<(), Error> {
        let event = diagnostics::redact_event(event);
        output.record(&event)?;
        let record = self.state.event(event);
        output.announce(&record, self.state.serial_verbosity);
        frontend.update(&self.state, Update::Record(record));
        Ok(())
    }

    /// Process queued events before clearing the active-write guard.
    async fn poll(
        &mut self,
        logs: &mut mpsc::UnboundedReceiver<Event>,
        frontend: &mut impl Frontend,
        output: &mut Output,
    ) -> Result<(), Error> {
        for _ in 0..EVENT_CAPACITY {
            let event = match self.rx.try_recv() {
                Ok(event) => event,
                Err(_) => match logs.try_recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            self.publish(event, frontend, output)?;
        }
        // Check task completion first: no new terminal event can arrive after this queue check.
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) && self.rx.is_empty() {
            if let Some(worker) = self.worker.take() {
                let error = match worker.await {
                    Ok(Ok(_)) => None,
                    Ok(Err(error)) => Some(format!("Installer worker failed: {error:#}")),
                    Err(error) => Some(format!("Installer worker terminated: {error}")),
                };
                if let Some(details) = error {
                    self.publish(
                        Event::Error {
                            details,
                            uncertain: true,
                        },
                        frontend,
                        output,
                    )?;
                }
            }
        }
        output.flush_mirrors();
        Ok(())
    }

    /// Power authorization is enforced here even if a frontend bypasses or has stale menus.
    fn can_power(&self) -> bool {
        self.state.can_power(self.active())
    }

    /// Explain a rejected power request without allowing presentation to clear uncertainty.
    fn block_power(
        &mut self,
        frontend: &mut impl Frontend,
        output: &mut Output,
    ) -> Result<(), Error> {
        frontend.update(
            &self.state,
            Update::Interrupted {
                active: self.active(),
            },
        );
        self.state.activity = "Servicing may still be active; power actions are unavailable until Trident confirms it is idle".into();
        self.publish(
            Event::InstallerLog {
                level: Level::Warn,
                message: self.state.activity.clone(),
            },
            frontend,
            output,
        )
    }
}

/// Runs the same servicing controller for TUI, plain, and headless operation.
/// Frontend failures return an error, never a retry or reboot; Trident may continue independently.
pub(crate) async fn run(
    config_path: PathBuf,
    frontend: &mut impl Frontend,
    mut output: Output,
) -> Result<(), Error> {
    let (log_tx, mut logs) = mpsc::unbounded_channel();
    INSTALLER_LOG_TX
        .set(log_tx)
        .map_err(|_| anyhow!("Installer log receiver already initialized"))?;
    let mut interrupts = unix::signal(SignalKind::interrupt())?;
    let mut termination = unix::signal(SignalKind::terminate())?;
    let mut session = Session::new(config_path);
    session.start(Request::Autorun)?;
    frontend.update(&session.state, Update::AttemptStarted);
    let mut shell: Option<JoinHandle<Result<(), Error>>> = None;
    loop {
        session.poll(&mut logs, frontend, &mut output).await?;
        if shell.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = shell.take() {
                if let Some(event) = shell_result_event(finished.await) {
                    session.publish(event, frontend, &mut output)?;
                }
            }
            frontend.resume()?;
        }
        if shell.is_none() {
            if session
                .state
                .reboot_at
                .is_some_and(|deadline| Instant::now() >= deadline)
                && session.can_power()
            {
                session.state.reboot_at = None;
                session.publish(
                    Event::InstallerLog {
                        level: Level::Info,
                        message: "Rebooting after successful installation".into(),
                    },
                    frontend,
                    &mut output,
                )?;
                match systemd::reboot() {
                    Ok(()) => return Ok(()),
                    Err(error) => session.publish(
                        Event::Error {
                            details: format!("Failed to reboot: {error:#}"),
                            uncertain: false,
                        },
                        frontend,
                        &mut output,
                    )?,
                }
            }
            if !frontend.interactive() {
                if !session.active()
                    && session.rx.is_empty()
                    && logs.is_empty()
                    && session.state.reboot_at.is_none()
                {
                    if session.state.failed {
                        return Err(anyhow!(
                            "{}\nRecovery requires an interactive terminal.",
                            session.state.details
                        ));
                    }
                    return Ok(());
                }
            } else {
                frontend.draw(&session.state)?;
                if let Some(command) = frontend.poll(&session.state, session.active())? {
                    match command {
                        Command::Start(_) | Command::Retry => {
                            let request = match command {
                                Command::Start(request) => request,
                                _ => session.state.last_request.clone(),
                            };
                            match session.start(request) {
                                Ok(()) => frontend.update(&session.state, Update::AttemptStarted),
                                Err(error) => session.publish(
                                    Event::Error {
                                        details: format!("{error:#}"),
                                        uncertain: true,
                                    },
                                    frontend,
                                    &mut output,
                                )?,
                            }
                        }
                        Command::InvalidInput(details) => session.publish(
                            Event::Error {
                                details,
                                uncertain: false,
                            },
                            frontend,
                            &mut output,
                        )?,
                        Command::Shell => {
                            frontend.suspend()?;
                            let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
                            shell = Some(task::spawn_blocking(move || terminal::shell(&tty)));
                        }
                        Command::Reboot | Command::Poweroff => {
                            if !session.can_power() {
                                session.block_power(frontend, &mut output)?;
                            } else {
                                let reboot = command == Command::Reboot;
                                let message = if reboot {
                                    "Reboot requested"
                                } else {
                                    "Shutdown requested"
                                };
                                session.publish(
                                    Event::InstallerLog {
                                        level: Level::Info,
                                        message: message.into(),
                                    },
                                    frontend,
                                    &mut output,
                                )?;
                                let result = if reboot {
                                    systemd::reboot()
                                } else {
                                    systemd::poweroff()
                                };
                                match result {
                                    Ok(()) => return Ok(()),
                                    Err(error) => session.publish(
                                        Event::Error {
                                            details: format!(
                                                "Failed to {}: {error:#}",
                                                if reboot { "reboot" } else { "shut down" }
                                            ),
                                            uncertain: false,
                                        },
                                        frontend,
                                        &mut output,
                                    )?,
                                }
                            }
                        }
                    }
                }
            }
        }
        tokio::select! {
            _ = time::sleep(TICK_INTERVAL) => {}
            signal = interrupts.recv(), if shell.is_none() => {
                if signal.is_none() { return Err(anyhow!("Installer interrupt handler closed")); }
                frontend.update(&session.state, Update::Interrupted { active: session.active() });
            }
            _ = termination.recv() => return Err(anyhow!("Installer stopped; servicing may continue in Trident")),
        }
    }
}

/// Shell exit failures are diagnostics, not the result of an independent servicing operation.
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use trident_client::{Completion, LogLevel, RebootDisposition, ServicingEvent, ServicingKind};

    use crate::{diagnostics::ResultKind, serial::SerialMirrors};

    /// Bounds scheduler coordination when checking late terminal messages.
    const WORKER_TEST_TIMEOUT: Duration = Duration::from_secs(1);

    /// A frontend with no terminal, proving orchestration does not depend on UI types.
    #[derive(Default)]
    struct RecordingFrontend {
        records: Vec<Record>,
    }

    impl Frontend for RecordingFrontend {
        /// Tests have no operator input or presentation-side recovery loop.
        fn interactive(&self) -> bool {
            false
        }

        /// Save only observable diagnostics, not the engine's mutable state.
        fn update(&mut self, _: &State, update: Update) {
            if let Update::Record(record) = update {
                self.records.push(record);
            }
        }

        /// Rendering is unnecessary to drive the real engine.
        fn draw(&mut self, _: &State) -> Result<(), Error> {
            Ok(())
        }

        /// A headless adapter has no commands.
        fn poll(&mut self, _: &State, _: bool) -> Result<Option<Command>, Error> {
            Ok(None)
        }

        /// No display is owned by this adapter.
        fn suspend(&mut self) -> Result<(), Error> {
            Ok(())
        }

        /// No display state needs restoring.
        fn resume(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    /// Diagnostic-only tests do not need a privileged log file or serial devices.
    fn output() -> Output {
        Output {
            log: None,
            mirrors: SerialMirrors::default(),
        }
    }

    /// A valid terminal response whose reboot is deliberately caller-managed.
    fn completed() -> Event {
        Event::Response(ServicingEvent::Completed(Ok(Completion {
            reboot: RebootDisposition::Required,
            servicing_kind: Some(ServicingKind::CleanInstall),
            image_hash: None,
        })))
    }

    /// Wait for task completion without making queue emptiness part of the waiting condition.
    async fn finished(session: &Session) {
        time::timeout(WORKER_TEST_TIMEOUT, async {
            while !session.worker.as_ref().unwrap().is_finished() {
                task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    /// Even a completed worker blocks power until its queued completion has been applied.
    #[tokio::test]
    async fn queued_completion_keeps_power_guard_active() {
        let mut session = Session::new("unused.toml".into());
        let tx = session.tx.clone();
        session.worker = Some(task::spawn(async move {
            tx.send(completed()).await.unwrap();
            Ok(AttemptOutcome::Completed)
        }));
        finished(&session).await;
        assert!(!session.rx.is_empty());
        assert!(!session.can_power());
        session.start(Request::Autorun).unwrap_err();
        let (_, mut logs) = mpsc::unbounded_channel();
        let mut frontend = RecordingFrontend::default();
        session
            .poll(&mut logs, &mut frontend, &mut output())
            .await
            .unwrap();
        assert!(!session.active());
        assert!(session.can_power());
        assert_eq!(
            frontend.records.last().unwrap().result,
            Some(ResultKind::Success)
        );
        assert!(session.state.reboot_at.is_some());
    }

    /// A terminal error behind a full batch must remain guarded across both poll iterations.
    #[tokio::test]
    async fn queued_disconnect_keeps_uncertain_writes_guarded() {
        let mut session = Session::new("unused.toml".into());
        let tx = session.tx.clone();
        session.worker = Some(task::spawn(async move {
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
            Ok(AttemptOutcome::Failed)
        }));
        time::timeout(WORKER_TEST_TIMEOUT, async {
            while session.rx.len() < EVENT_CAPACITY {
                task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (_, mut logs) = mpsc::unbounded_channel();
        let mut frontend = RecordingFrontend::default();
        session
            .poll(&mut logs, &mut frontend, &mut output())
            .await
            .unwrap();
        assert!(!session.can_power());
        finished(&session).await;
        session
            .poll(&mut logs, &mut frontend, &mut output())
            .await
            .unwrap();
        assert!(!session.active());
        assert!(session.state.uncertain);
        assert!(!session.can_power());
        assert!(session.state.reboot_at.is_none());
    }

    /// A worker without a handled outcome is uncertain even if its queue is empty.
    #[tokio::test]
    async fn unexpected_worker_error_never_releases_power() {
        let mut session = Session::new("unused.toml".into());
        session.worker = Some(task::spawn(async { Err(anyhow!("worker failed")) }));
        finished(&session).await;
        let (_, mut logs) = mpsc::unbounded_channel();
        let mut frontend = RecordingFrontend::default();
        session
            .poll(&mut logs, &mut frontend, &mut output())
            .await
            .unwrap();
        assert!(!session.active());
        assert!(!session.can_power());
        assert!(session.state.details.contains("worker failed"));
        assert_eq!(
            frontend.records.last().unwrap().result,
            Some(ResultKind::Failure)
        );
    }

    /// Retry intent and uncertainty live in the engine, not in whichever screen requests them.
    #[test]
    fn retry_state_preserves_the_original_source_and_uncertainty() {
        let request =
            Request::HostConfiguration("https://example.test/host.yaml?sig=secret".into());
        let mut state = State::new();
        state.fail("Disconnected".into(), true);
        state.begin(request.clone());
        assert_eq!(state.last_request, request);
        assert!(state.uncertain);
        assert!(!state.can_power(false));
    }

    /// Shell warnings must not change completed servicing or its scheduled reboot.
    #[test]
    fn shell_failure_does_not_replace_a_successful_result() {
        let mut state = State::new();
        state.event(completed());
        let deadline = state.reboot_at;
        let warning = state.event(shell_result_event(Ok(Err(anyhow!("exit 1")))).unwrap());
        assert_eq!(warning.level, LogLevel::Warn);
        assert!(!state.failed);
        assert_eq!(state.reboot_at, deadline);
    }

    /// Real configuration failure reaches a terminal-free adapter without any UI or live install.
    #[tokio::test]
    async fn headless_engine_reports_configuration_failure_without_a_terminal() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("installer.toml");
        fs::write(&path, "mode = 'not-a-mode'").unwrap();
        let mut frontend = RecordingFrontend::default();
        let error = run(path, &mut frontend, output()).await.unwrap_err();
        assert!(error.to_string().contains("not-a-mode"), "{error:#}");
        assert!(error
            .to_string()
            .contains("Recovery requires an interactive terminal"));
        assert_eq!(
            frontend.records.last().unwrap().result,
            Some(ResultKind::Failure)
        );
    }

    /// Recovery edits are reloaded by a new worker, not cached in presentation state.
    #[tokio::test]
    async fn repeated_attempts_reload_the_same_bootstrap_path() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("installer.toml");
        let mut session = Session::new(path.clone());
        let (_, mut logs) = mpsc::unbounded_channel();
        let mut frontend = RecordingFrontend::default();
        for invalid_mode in ["first-invalid-mode", "second-invalid-mode"] {
            fs::write(&path, format!("mode = '{invalid_mode}'")).unwrap();
            session.start(Request::Autorun).unwrap();
            time::timeout(WORKER_TEST_TIMEOUT, async {
                while session.active() {
                    session
                        .poll(&mut logs, &mut frontend, &mut output())
                        .await
                        .unwrap();
                    task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                session.state.details.contains(invalid_mode),
                "{}",
                session.state.details
            );
            assert!(session.state.failed);
            assert!(session.can_power());
        }
    }
}
