use std::{
    env::{self, VarError},
    fs,
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{bail, Context, Error};
use log::{error, info, LevelFilter, Log, Metadata, Record};

mod activation;
mod config;
mod manifest;
mod release;
mod tools;

struct Stderr;

impl Log for Stderr {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        eprintln!("trident-slot-loader: {}: {}", record.level(), record.args());
    }

    fn flush(&self) {}
}

fn main() -> ExitCode {
    if let Err(error) = log::set_logger(&Stderr) {
        eprintln!("trident-slot-loader: ERROR: {error}");
        return ExitCode::FAILURE;
    }
    log::set_max_level(LevelFilter::Info);
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Error> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let recover = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--recover" => true,
        [argument] if argument == "--help" => {
            println!("Usage: trident-slot-loader [--recover | --version | --help]");
            return Ok(());
        }
        [argument] if argument == "--version" => {
            println!("trident-slot-loader {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => bail!("expected no arguments, --recover, --version or --help"),
    };
    let config = config::Config::new(
        PathBuf::from("/sysroot"),
        PathBuf::from("/run"),
        env::var_os("TRIDEN_SLOT_STORE"),
    )?;
    if !config.store_present()? {
        info!("store absent: {}", config.store().display());
        return Ok(());
    }
    let cmdline = fs::read_to_string("/proc/cmdline").context("reading kernel command line")?;
    let slot = config::Slot::parse(&cmdline)?;
    let slot_dir = config.slot_dir(slot)?;
    if recover {
        let enabled =
            config::reboot_enabled(Some(&setting("REBOOT_ON_UPDATE_LOADER_ERROR", "true")?))?;
        return activation::recover(&slot_dir, enabled, &tools::System);
    }
    let mode = setting("LOADER_MODE", "acl")?;
    anyhow::ensure!(mode == "acl", "unsupported LOADER_MODE {mode:?}");
    let confext_mode = setting("CONFEXT_MODE", "overlay")?;
    let confext_mode = config::ConfextMode::parse(&confext_mode)?;
    let manifest = manifest::Manifest::read(slot_dir.join("slot-loader.json"))?;
    info!(
        "slot={} manifest={} backend={confext_mode:?}",
        slot.name(),
        slot_dir.join("slot-loader.json").display()
    );
    activation::activate(&config, slot, &manifest, confext_mode, &tools::System)?;
    activation::clear_pending(&slot_dir)?;
    info!("slot={} prepared", slot.name());
    Ok(())
}

fn setting(name: &str, default: &str) -> Result<String, Error> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(VarError::NotPresent) => Ok(default.to_string()),
        Err(error) => Err(error).with_context(|| format!("reading {name}")),
    }
}
