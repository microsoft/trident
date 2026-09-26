//! The `tailor` binary entry point: parse args, initialize logging, dispatch, map to an exit code.

mod cli;
mod error;
mod run;
mod scaffold;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::{
    EnvFilter, fmt,
    fmt::{format::Writer, time::FormatTime},
};

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    run::init_timestamps(cli.timestamps);
    init_tracing(cli.verbose, cli.quiet);

    match run::dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let label = if run::use_color() {
                "\u{1b}[1;31merror:\u{1b}[0m"
            } else {
                "error:"
            };
            eprintln!("{label} {error}");
            // Walk the cause chain so wrapped diagnostics (e.g. the serde field that failed to parse)
            // are visible, not just the top-level "failed to parse …" summary.
            let mut cause = std::error::Error::source(&error);
            while let Some(source) = cause {
                eprintln!("  caused by: {source}");
                cause = source.source();
            }
            ExitCode::from(error.exit_code())
        }
    }
}

/// A compact timer for the live `tracing` view (`meta/docs/2026-06-29-logging.md` §5.6). It reads the same
/// process-global mode and zero point as cargo-style status lines so both streams agree.
struct CompactTime;

impl FormatTime for CompactTime {
    fn format_time(&self, writer: &mut Writer<'_>) -> std::fmt::Result {
        write!(writer, "{}", run::timestamp_prefix())
    }
}

/// Initialize tracing from `-v`/`-q` flags, overridable by `RUST_LOG`.
fn init_tracing(verbose: u8, quiet: u8) {
    let level = match i16::from(verbose) - i16::from(quiet) {
        i16::MIN..=-1 => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(run::use_color())
        .with_timer(CompactTime)
        .init();
}
