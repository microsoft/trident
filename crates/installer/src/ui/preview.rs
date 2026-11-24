//! Development-only replay through the live model and renderer; no servicing or power calls.

use std::{
    io::{self, IsTerminal},
    time::{Duration, Instant},
    vec::IntoIter,
};

use anyhow::{bail, ensure, Context, Error};
use clap::ValueEnum;
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEventKind};
use log::Level;
use tokio::{signal, time};

use trident_client::{
    Completion, LogLevel, LogRecord, RebootDisposition, RemoteError, RemoteErrorCategory,
    ServicingEvent, ServicingKind,
};

use crate::{client::Event, config::SerialVerbosity, ui};

use super::{
    model::{Action, Model, Screen},
    render, Display, FRAME_INTERVAL, INPUT_EVENTS_PER_FRAME,
};

/// Keep replay slow enough to change verbosity while messages are arriving.
const RECORD_INTERVAL: Duration = Duration::from_millis(150);
/// Real VM log excerpts plus command-trace samples, with machine-specific values removed.
const LOG_SAMPLE: &str = include_str!("preview-logs.json");

/// Selects the final servicing result after the same sample log stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Scenario {
    Success,
    Error,
}

/// Runs on the caller's terminal; Q exits without starting any live installer setup.
pub(crate) async fn run(scenario: Scenario) -> Result<(), Error> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "--ui-preview requires an interactive terminal"
    );
    let mut model = Model::new();
    let mut pending = replay(scenario)?;
    let mut next_record = Instant::now();
    let mut active = true;
    let mut display = Display::new(false)?;
    loop {
        if active && Instant::now() >= next_record {
            match pending.next() {
                Some(event) => {
                    model.event(ui::redact_event(event));
                }
                None => active = false,
            }
            next_record = Instant::now() + RECORD_INTERVAL;
        }
        display
            .terminal
            .draw(|frame| render::render(frame, &model))?;
        for _ in 0..INPUT_EVENTS_PER_FRAME {
            if !event::poll(Duration::ZERO)? {
                break;
            }
            let TerminalEvent::Key(key) = event::read()? else {
                continue;
            };
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            if !model.verbosity_open
                && !matches!(model.screen, Screen::StreamUrl | Screen::HostConfigUrl)
                && matches!(key.code, KeyCode::Char('q' | 'Q'))
            {
                return Ok(());
            }
            if let Some(action) = model.key(key, active) {
                handle_action(&mut model, action, &mut active, &mut pending, scenario)?;
            }
        }
        tokio::select! {
            _ = time::sleep(FRAME_INTERVAL) => {}
            result = signal::ctrl_c() => {
                result?;
                return Ok(());
            }
        }
    }
}

/// Recovery sources restart the sample; machine actions only add a visible notice.
fn handle_action(
    model: &mut Model,
    action: Action,
    active: &mut bool,
    pending: &mut IntoIter<Event>,
    scenario: Scenario,
) -> Result<(), Error> {
    match action {
        Action::Start(_) if !*active => {
            *pending = replay(scenario)?;
            *active = true;
            model.screen = Screen::Progress;
            model.started = Instant::now();
            model.selected = 0;
            model.scroll = 0;
        }
        Action::Start(_) => {
            model.event(Event::InstallerLog {
                level: Level::Warn,
                message: "UI PREVIEW: wait for the current replay to finish".into(),
            });
        }
        Action::Shell | Action::Reboot | Action::Poweroff | Action::BlockedPower => {
            let message = format!("UI PREVIEW: {action:?} is simulated");
            model.source = format!("{message} - Q: quit");
            if model.screen == Screen::Recovery {
                model.details = message.clone();
            }
            model.event(Event::InstallerLog {
                level: Level::Info,
                message,
            });
        }
    }
    Ok(())
}

/// Builds the same domain events the live gRPC worker passes to the UI.
fn replay(scenario: Scenario) -> Result<IntoIter<Event>, Error> {
    let mut events = vec![
        Event::Prepared {
            description: format!("UI PREVIEW ({scenario:?}) - Q: quit"),
            reboot: false,
            serial_verbosity: SerialVerbosity::Debug,
        },
        Event::Response(ServicingEvent::Started { timestamp: None }),
    ];
    let records: Vec<(String, String)> =
        serde_json::from_str(LOG_SAMPLE).context("Invalid UI preview log sample")?;
    for (level, message) in records {
        let level = match level.as_str() {
            "trace" => LogLevel::Trace,
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" => LogLevel::Warn,
            other => bail!("Unsupported UI preview log level '{other}'"),
        };
        events.push(Event::Response(ServicingEvent::Log(LogRecord {
            timestamp: None,
            level,
            message,
            module: String::new(),
            target: String::new(),
            location: None,
        })));
    }
    let completed = match scenario {
        Scenario::Success => Ok(Completion {
            reboot: RebootDisposition::Required,
            servicing_kind: Some(ServicingKind::CleanInstall),
            image_hash: None,
        }),
        Scenario::Error => Err(RemoteError {
            category: RemoteErrorCategory::Servicing,
            subkind: "write-netplan-config".into(),
            message: concat!(
                "Trident failed due to a servicing error\n\nContext:\n",
                "    0: Failed to execute a clean install\n",
                "    1: Failed to execute in chroot\n",
                "    2: Step 'Configure' failed for subsystem 'network'\n",
                "    3: Failed to write Netplan config\n",
                "    4: Failed to open Netplan configuration file /etc/netplan/99-trident.yaml\n",
                "    5: No such file or directory (os error 2)"
            )
            .into(),
            error_message: "Failed to write Netplan config".into(),
            location: None,
        }),
    };
    events.push(Event::Response(ServicingEvent::Completed(completed)));
    Ok(events.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyEvent, KeyModifiers};
    use log::LevelFilter;
    use ratatui::{backend::TestBackend, Terminal};

    /// Exercise the live verbosity picker rather than setting its filter directly.
    fn verbosity(model: &mut Model, digit: char) {
        for code in [KeyCode::Char('v'), KeyCode::Char(digit), KeyCode::Enter] {
            model.key(KeyEvent::new(code, KeyModifiers::NONE), true);
        }
    }

    /// Sample completions use the real model without enabling automatic machine actions.
    #[test]
    fn replay_reaches_both_results_without_scheduling_reboot() {
        for (scenario, screen) in [
            (Scenario::Success, Screen::Success),
            (Scenario::Error, Screen::Error),
        ] {
            let mut model = Model::new();
            for event in replay(scenario).unwrap() {
                model.event(ui::redact_event(event));
            }
            assert_eq!(model.screen, screen);
            assert!(model.reboot_at.is_none());
            if scenario == Scenario::Error {
                assert!(model.details.contains("No such file or directory"));
            }
        }
    }

    /// Raising verbosity must reveal retained records rather than leave blank history.
    #[test]
    fn info_to_trace_renders_previously_hidden_history() {
        for (width, height) in [(80, 24), (80, 40), (160, 50)] {
            let mut model = Model::new();
            verbosity(&mut model, '4');
            assert_eq!(model.verbosity, LevelFilter::Info);
            for event in replay(Scenario::Success).unwrap().take(32) {
                model.event(ui::redact_event(event));
            }
            let history = model
                .logs
                .iter()
                .map(|entry| entry.message.clone())
                .collect::<Vec<_>>();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let text = |terminal: &Terminal<TestBackend>| {
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            };
            model.scroll = usize::MAX;
            terminal
                .draw(|frame| render::render(frame, &model))
                .unwrap();
            assert!(!text(&terminal).contains("Executing 'lsblk"));
            verbosity(&mut model, '6');
            assert_eq!(model.verbosity, LevelFilter::Trace);
            assert_eq!(
                model
                    .logs
                    .iter()
                    .map(|entry| entry.message.clone())
                    .collect::<Vec<_>>(),
                history
            );
            model.scroll = usize::MAX;
            terminal
                .draw(|frame| render::render(frame, &model))
                .unwrap();
            let rendered = text(&terminal);
            assert!(
                rendered.contains("Loading datastore"),
                "{width}x{height}: {rendered}"
            );
            assert!(
                rendered.contains("Executing 'lsblk"),
                "{width}x{height}: {rendered}"
            );
        }
    }

    /// Preview action handling must stay on the notice-only path for shell and power requests.
    #[test]
    fn machine_actions_are_replaced_with_notices() {
        let mut model = Model::new();
        let mut active = false;
        let mut pending = replay(Scenario::Success).unwrap();
        for action in [Action::Shell, Action::Reboot, Action::Poweroff] {
            handle_action(
                &mut model,
                action,
                &mut active,
                &mut pending,
                Scenario::Success,
            )
            .unwrap();
            assert!(model.logs.back().unwrap().message.contains("is simulated"));
            assert!(!active);
        }
    }
}
