//! Initrd entry point: configure diagnostics, select a command and delegate its workflow.

use std::{env, process::ExitCode};

use anyhow::{bail, Error};
use log::info;

mod config;
mod diagnostics;
mod filesystem;
mod loader;
mod manifest;
mod mounts;
mod outputs;
mod recovery;
mod release;
mod selection;
mod symlinks;
mod tools;

const CMDLINE_PATH: &str = "/proc/cmdline";
const MAX_CMDLINE_BYTES: u64 = 64 * 1024;

fn main() -> ExitCode {
    let result = config::log_level_from_environment()
        .and_then(diagnostics::init)
        .and_then(|_| run());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            diagnostics::report_failure(&error);
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
    let paths = diagnostics::stage("configuration", config::Paths::from_environment)?;
    info!(
        "event=command_start version={} recover={recover} store={} host_store={}",
        env!("CARGO_PKG_VERSION"),
        paths.store().display(),
        paths.host_store().display()
    );
    if !diagnostics::stage("store_lookup", || {
        filesystem::directory_exists(paths.store())
    })? {
        info!("event=loader_noop reason=store_absent");
        return Ok(());
    }
    let slot = diagnostics::stage("slot_detection", || {
        selection::Slot::parse(&filesystem::read_bounded(CMDLINE_PATH, MAX_CMDLINE_BYTES)?)
    })?;
    let slot_dir = paths.store().join(slot.name());
    filesystem::directory(&slot_dir)?;
    info!("event=slot_selected slot={slot}");
    if recover {
        let enabled =
            diagnostics::stage("recovery_configuration", config::recovery_from_environment)?;
        diagnostics::stage("recovery", || {
            recovery::run(&slot_dir, enabled, &tools::System)
        })
    } else {
        let mode = diagnostics::stage(
            "activation_configuration",
            config::activation_from_environment,
        )?;
        info!(
            "event=configuration_ready slot={slot} confext_mode={mode} sysroot={} runtime={}",
            paths.sysroot().display(),
            paths.runtime().display()
        );
        loader::run(&paths, slot, mode, &tools::System)
    }
}
