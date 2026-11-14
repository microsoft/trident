use std::{
    fs::{self, OpenOptions, Permissions},
    io::{self, IsTerminal, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::ExitCode,
    sync::OnceLock,
};

use anyhow::{Context, Error};
use clap::Parser;
use log::{warn, LevelFilter, Record};
use tokio::runtime::Builder;
use tokio::sync::mpsc::UnboundedSender;

use osutils::{systemd, terminal};

use crate::{client::Event, config::DEFAULT_CONFIG, ui::Output};

mod client;
mod config;
mod source;
mod ui;

const LOG_PATH: &str = "/var/log/trident-installer.log";
const PRIVATE_FILE_MODE: u32 = 0o600;
const WORKER_THREADS: usize = 2;
const APPLICATION_NAME: &str = "Trident Linux Installer";
static INSTALLER_LOG_TX: OnceLock<UnboundedSender<Event>> = OnceLock::new();

#[derive(Debug, Parser)]
#[command(
    name = "trident-installer",
    version,
    about = APPLICATION_NAME
)]
struct Args {
    #[arg(long, default_value = DEFAULT_CONFIG)]
    config: PathBuf,
    #[arg(long)]
    plain: bool,
    #[arg(long)]
    system_console: bool,
}

/// Starts logging and the installer runtime, returning a failing exit code on startup errors.
fn main() -> ExitCode {
    let stderr_is_terminal = io::stderr().is_terminal();
    env_logger::Builder::from_default_env()
        .filter_level(LevelFilter::Info)
        .filter_module("installer", LevelFilter::Trace)
        .filter_module("osutils", LevelFilter::Trace)
        .format(move |buf, record| {
            format_log_record(buf, record, INSTALLER_LOG_TX.get(), stderr_is_terminal)
        })
        .init();
    let args = Args::parse();
    let result = Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .enable_all()
        .build()
        .context("Failed to create installer async runtime")
        .and_then(|runtime| runtime.block_on(run(args)));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Installer failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Keeps terminal-bound logs in Ratatui while retaining startup and journal diagnostics.
fn format_log_record(
    output: &mut impl Write,
    record: &Record<'_>,
    log_tx: Option<&UnboundedSender<Event>>,
    stderr_is_terminal: bool,
) -> io::Result<()> {
    let message = record.args().to_string();
    if let Some(tx) = log_tx {
        if tx
            .send(Event::InstallerLog {
                level: record.level(),
                message: message.clone(),
            })
            .is_ok()
        {
            if stderr_is_terminal {
                return Ok(());
            }
        } else {
            writeln!(
                output,
                "Installer log receiver unavailable; entry retained in journal"
            )?;
        }
    }
    writeln!(
        output,
        "[{} {}] {}",
        record.level(),
        record.target(),
        message
    )
}

/// Sets up private diagnostics and optional console ownership before starting the UI.
async fn run(args: Args) -> Result<(), Error> {
    let mut linux_vt = false;
    let mut output = Output {
        log: None,
        mirrors: Vec::new(),
    };
    output.log = Some(
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(PRIVATE_FILE_MODE)
            .open(LOG_PATH)
            .context("Failed to open private installer diagnostic log; run as root")?,
    );
    fs::set_permissions(LOG_PATH, Permissions::from_mode(PRIVATE_FILE_MODE))?;
    if args.system_console {
        systemd::start_unit("tridentd.socket")?;
        systemd::start_unit("systemd-udev-settle.service")?;
        let consoles = terminal::active_consoles()?;
        if let Some(graphical) = consoles.iter().find(|path| path.ends_with("tty1")) {
            systemd::stop_unit("getty@tty1.service")?;
            let _terminal = terminal::attach(graphical)?;
            linux_vt = true;
        }
        for path in consoles.iter().filter(|path| !path.ends_with("tty1")) {
            match terminal::open_nonblocking_console(path) {
                Ok(terminal) => output.mirrors.push((path.clone(), terminal)),
                Err(error) => warn!("Could not mirror status to '{}': {error}", path.display()),
            }
        }
    }
    ui::run(args.config, args.plain, linux_vt, output).await
}

#[cfg(test)]
mod tests {
    use super::*;

    use log::Level;
    use tokio::sync::mpsc;

    #[test]
    fn terminal_logs_use_ui_events_while_journal_and_startup_logs_remain_visible() {
        let record = Record::builder()
            .args(format_args!("Installer progress"))
            .level(Level::Info)
            .target("installer")
            .build();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut output = Vec::new();
        format_log_record(&mut output, &record, Some(&tx), true).unwrap();
        assert!(output.is_empty());
        assert!(matches!(
            rx.try_recv().unwrap(),
            Event::InstallerLog {
                level: Level::Info,
                message
            } if message == "Installer progress"
        ));

        format_log_record(&mut output, &record, Some(&tx), false).unwrap();
        assert_eq!(output, b"[INFO installer] Installer progress\n");
        output.clear();

        format_log_record(&mut output, &record, None, true).unwrap();
        assert_eq!(output, b"[INFO installer] Installer progress\n");
        output.clear();

        drop(rx);
        format_log_record(&mut output, &record, Some(&tx), true).unwrap();
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("Installer log receiver unavailable"));
    }
}
