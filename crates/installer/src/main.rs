use std::{
    fs::{self, OpenOptions, Permissions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context, Error};
use clap::Parser;
use log::LevelFilter;
use tokio::runtime::Builder;

use osutils::{systemd, terminal};

use crate::{config::DEFAULT_CONFIG, demo::Scenario, ui::Output};

mod client;
mod config;
mod demo;
mod source;
mod ui;

const LOG_PATH: &str = "/var/log/trident-installer.log";
const PRIVATE_FILE_MODE: u32 = 0o600;
const WORKER_THREADS: usize = 2;
const APPLICATION_NAME: &str = "Trident Linux Installer";

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
    let mut output = Output {
        log: None,
        mirrors: Vec::new(),
        control: "current terminal".into(),
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
            let primary = terminal::preferred_console(&consoles)?;
            let name = primary
                .file_name()
                .context("Console has no device name")?
                .to_string_lossy();
            let unit =
                if name.starts_with("tty") && name.chars().skip(3).all(|c| c.is_ascii_digit()) {
                    format!("getty@{name}.service")
                } else {
                    format!("serial-getty@{name}.service")
                };
            systemd::stop_unit(&unit)?;
            let _terminal = terminal::attach(primary)?;
            output.control = primary.display().to_string();
            for path in consoles.iter().filter(|path| path.as_path() != primary) {
                output
                    .mirrors
                    .push(OpenOptions::new().write(true).open(path).with_context(|| {
                        format!("Failed to open active console '{}'", path.display())
                    })?);
            }
        }
    }
    let config_path = args.demo.is_none().then_some(args.config);
    ui::run(config_path, args.demo, args.plain, output).await
}
