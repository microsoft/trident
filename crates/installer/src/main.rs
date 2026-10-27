use std::{
    fs::{self, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::ExitCode,
    sync::OnceLock,
};

use anyhow::{Context, Error};
use clap::Parser;
use log::{warn, LevelFilter};
use tokio::runtime::Builder;
use tokio::sync::mpsc::UnboundedSender;

use osutils::{systemd, terminal};

use crate::{client::Event, config::DEFAULT_CONFIG, demo::Scenario, ui::Output};

mod client;
mod config;
mod demo;
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
    #[arg(long, num_args = 0..=1, default_missing_value = "success", value_enum)]
    demo: Option<Scenario>,
    #[arg(long)]
    plain: bool,
    #[arg(long, conflicts_with = "demo")]
    system_console: bool,
}

fn main() -> ExitCode {
    env_logger::Builder::from_default_env()
        .filter_level(LevelFilter::Info)
        .filter_module("installer", LevelFilter::Trace)
        .filter_module("osutils", LevelFilter::Trace)
        .format(|buf, record| {
            let message = record.args().to_string();
            if let Some(tx) = INSTALLER_LOG_TX.get() {
                if tx
                    .send(Event::InstallerLog {
                        level: record.level(),
                        message: message.clone(),
                    })
                    .is_err()
                {
                    writeln!(
                        buf,
                        "Installer log receiver unavailable; entry retained in journal"
                    )?;
                }
            }
            writeln!(buf, "[{} {}] {}", record.level(), record.target(), message)
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

async fn run(args: Args) -> Result<(), Error> {
    let mut linux_vt = false;
    let mut output = Output {
        log: None,
        mirrors: Vec::new(),
    };
    if args.demo.is_none() {
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
            let consoles = terminal::active_consoles()?;
            if let Some(graphical) = consoles.iter().find(|path| path.ends_with("tty1")) {
                systemd::stop_unit("getty@tty1.service")?;
                let _terminal = terminal::attach(graphical)?;
                linux_vt = true;
            }
            for path in consoles.iter().filter(|path| !path.ends_with("tty1")) {
                match OpenOptions::new().write(true).open(path) {
                    Ok(terminal) => output.mirrors.push((path.clone(), terminal)),
                    Err(error) => warn!("Could not mirror status to '{}': {error}", path.display()),
                }
            }
        }
    }
    let config_path = args.demo.is_none().then_some(args.config);
    ui::run(config_path, args.demo, args.plain, linux_vt, output).await
}
