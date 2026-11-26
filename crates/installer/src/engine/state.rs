//! Servicing state and policy, with no terminal or screen dependencies.

use std::time::{Duration, Instant};

use log::{Level, LevelFilter};

use trident_client::{LogLevel, ServicingEvent};

use crate::{
    client::{self, Completion, Event},
    config::SerialVerbosity,
    diagnostics::{self, Record, ResultKind, Source},
    source::Request,
};

/// Give operators time to remove installer media before the automatic reboot.
const REBOOT_DELAY: Duration = Duration::from_secs(5);

/// Current installation state, shared with frontends as a read-only snapshot.
#[derive(Debug, Clone)]
pub(crate) struct State {
    /// Source description with URL secrets removed before presentation.
    pub(crate) source: String,
    /// Most recent preparation or servicing activity.
    pub(crate) activity: String,
    /// Complete current result, also returned by headless failures.
    pub(crate) details: String,
    /// Retry repeats this request after reloading bootstrap settings.
    pub(crate) last_request: Request,
    /// Independent serial filter from the effective installer settings.
    pub(crate) serial_verbosity: LevelFilter,
    /// The daemon may still be writing; only an idle confirmation or valid completion clears it.
    pub(crate) uncertain: bool,
    /// Effective automatic reboot policy, never a request for the daemon to reboot.
    reboot: bool,
    /// Reboot deadline after completed work requiring reboot; suspended while Shell is open.
    pub(crate) reboot_at: Option<Instant>,
    /// Start of the current attempt, used for elapsed diagnostics.
    pub(crate) started: Instant,
    /// Whether the current result requires recovery or a failing headless exit.
    pub(crate) failed: bool,
}

impl State {
    /// Initial autorun state; settings may update its reboot and serial policies.
    pub(crate) fn new() -> Self {
        Self {
            source: String::new(),
            activity: "Preparing installer".into(),
            details: String::new(),
            last_request: Request::Autorun,
            serial_verbosity: SerialVerbosity::Debug.filter(),
            uncertain: false,
            reboot: true,
            reboot_at: None,
            started: Instant::now(),
            failed: false,
        }
    }

    /// Starts another attempt without clearing uncertainty about an earlier daemon write.
    pub(super) fn begin(&mut self, request: Request) {
        self.last_request = request;
        self.activity = "Preparing installation".into();
        self.started = Instant::now();
        self.reboot_at = None;
    }

    /// Power is safe only after the worker and its queued results finish, with no uncertainty.
    pub(crate) fn can_power(&self, worker_active: bool) -> bool {
        !worker_active && !self.uncertain
    }

    /// Records failure without claiming that a potentially disconnected daemon is idle.
    pub(crate) fn fail(&mut self, details: String, uncertain: bool) -> Record {
        self.details = details.clone();
        self.uncertain |= uncertain;
        self.reboot_at = None;
        self.failed = true;
        self.result_log(
            LogLevel::Error,
            format!("FAILURE: {details}"),
            ResultKind::Failure,
        )
    }

    /// Creates a complete sanitized record; retention and filtering belong to its consumers.
    pub(crate) fn log(&self, source: Source, level: LogLevel, message: String) -> Record {
        Record {
            elapsed: self.started.elapsed(),
            source,
            level,
            message: diagnostics::display_text(&message),
            result: None,
        }
    }

    /// Marks a terminal diagnostic so frontends can highlight it independently of log filters.
    fn result_log(&self, level: LogLevel, message: String, result: ResultKind) -> Record {
        let mut record = self.log(Source::Inst, level, message);
        record.result = Some(result);
        record
    }

    /// Applies a sanitized worker event and returns its complete diagnostic record.
    /// Invalid completions preserve uncertainty; only known completion or idleness clears it.
    pub(crate) fn event(&mut self, event: Event) -> Record {
        match event {
            Event::InstallerLog { level, message } => self.log(
                Source::Inst,
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
                self.log(Source::Inst, LogLevel::Info, activity)
            }
            Event::Prepared {
                description,
                reboot,
                serial_verbosity,
            } => {
                self.source = description.clone();
                self.reboot = reboot;
                self.serial_verbosity = serial_verbosity.filter();
                self.log(Source::Inst, LogLevel::Info, description)
            }
            Event::IdleConfirmed => {
                self.uncertain = false;
                self.log(
                    Source::Inst,
                    LogLevel::Info,
                    "Trident confirmed that no servicing is active".into(),
                )
            }
            Event::Error { details, uncertain } => self.fail(details, uncertain),
            Event::Response(response) => match response {
                ServicingEvent::Started { .. } => {
                    self.activity = "Servicing started".into();
                    self.log(Source::Inst, LogLevel::Info, self.activity.clone())
                }
                ServicingEvent::Log(log) => {
                    let level = log.level;
                    if level == LogLevel::Info {
                        self.activity = diagnostics::display_text(&log.message);
                    }
                    self.log(Source::Trident, level, log.message)
                }
                ServicingEvent::Completed(completed) => match client::completion(completed) {
                    Ok(Completion::Success {
                        reboot_required,
                        performed,
                    }) => {
                        self.uncertain = false;
                        self.failed = false;
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
                            ResultKind::Success,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_client::{Completion as ClientCompletion, RebootDisposition, ServicingKind};

    /// The same completion policy drives headless and graphical sessions.
    #[test]
    fn automatic_reboot_requires_policy_work_and_daemon_requirement() {
        for (enabled, performed, required) in [
            (true, true, true),
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let mut state = State::new();
            state.event(Event::Prepared {
                description: "Test install".into(),
                reboot: enabled,
                serial_verbosity: SerialVerbosity::Debug,
            });
            let record = state.event(Event::Response(ServicingEvent::Completed(Ok(
                ClientCompletion {
                    reboot: if required {
                        RebootDisposition::Required
                    } else {
                        RebootDisposition::NotRequired
                    },
                    servicing_kind: Some(if performed {
                        ServicingKind::CleanInstall
                    } else {
                        ServicingKind::NoneRequired
                    }),
                    image_hash: None,
                },
            ))));
            assert_eq!(record.result, Some(ResultKind::Success));
            assert_eq!(state.reboot_at.is_some(), enabled && performed && required);
        }
    }

    /// Neither a new attempt nor an ordinary failure proves a disconnected daemon has stopped.
    #[test]
    fn uncertainty_survives_retry_until_confirmed_idle() {
        let mut state = State::new();
        state.fail("Disconnected".into(), true);
        state.begin(Request::Autorun);
        state.fail("Invalid configuration".into(), false);
        assert!(state.uncertain);
        assert!(!state.can_power(false));
        state.event(Event::IdleConfirmed);
        assert!(state.can_power(false));
        assert!(!state.can_power(true));
    }
}
