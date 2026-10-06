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

/// Chatty infrastructure crates whose debug/trace output drowns tailor's own logs at `-vv`/`-vvv`:
/// the Docker client (`bollard`) and the HTTP plumbing under it (`hyper_util`). Pinned to `warn` in
/// the default filter so verbose runs stay about tailor and Image Customizer. `RUST_LOG` overrides
/// everything (including re-enabling these), and a future `--debug-internal` flag can opt back in.
const NOISY_DEPS: &[&str] = &["bollard", "hyper_util"];

/// The default `EnvFilter` directive string for a verbosity, used when `RUST_LOG` is unset. The
/// global level comes from `-v`/`-q`; chatty infra deps ([`NOISY_DEPS`]) are pinned to `warn` only
/// when the global level is more verbose than `warn`, so quieter runs are never made noisier.
fn default_filter_directives(verbose: u8, quiet: u8) -> String {
    let diff = i16::from(verbose) - i16::from(quiet);
    let level = match diff {
        i16::MIN..=-1 => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let mut directives = String::from(level);
    if diff >= 1 {
        for dep in NOISY_DEPS {
            directives.push(',');
            directives.push_str(dep);
            directives.push_str("=warn");
        }
    }
    directives
}

/// Initialize tracing from `-v`/`-q` flags, overridable by `RUST_LOG`.
fn init_tracing(verbose: u8, quiet: u8) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter_directives(verbose, quiet)));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(run::use_color())
        .with_timer(CompactTime)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_pins_infra_deps_to_warn_only_when_verbose() {
        // Default (-/warn) and quieter: no per-dep directives, so nothing is made noisier.
        assert_eq!(default_filter_directives(0, 0), "warn");
        assert_eq!(default_filter_directives(0, 1), "error");

        // Verbose: the global level climbs but the chatty infra crates stay at warn.
        assert_eq!(
            default_filter_directives(1, 0),
            "info,bollard=warn,hyper_util=warn"
        );
        assert_eq!(
            default_filter_directives(2, 0),
            "debug,bollard=warn,hyper_util=warn"
        );
        assert_eq!(
            default_filter_directives(3, 0),
            "trace,bollard=warn,hyper_util=warn"
        );
    }
}
