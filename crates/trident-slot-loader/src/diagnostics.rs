//! Console-first diagnostics with journald priority prefixes and monotonic stage timing.

use std::time::Instant;

use anyhow::{Context, Error};
use log::{error, info, Level, LevelFilter, Log, Metadata, Record};

struct Stderr;

impl Log for Stderr {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        eprintln!("{}", format_record(record));
    }

    fn flush(&self) {}
}

pub(super) fn init(level: LevelFilter) -> Result<(), Error> {
    log::set_logger(&Stderr).map_err(|error| anyhow::anyhow!("initializing logger: {error}"))?;
    log::set_max_level(level);
    Ok(())
}

fn format_record(record: &Record) -> String {
    format_line(record.level(), &record.args().to_string())
}

fn format_line(level: Level, message: &str) -> String {
    let priority = match level {
        Level::Error => 3,
        Level::Warn => 4,
        Level::Info => 6,
        Level::Debug | Level::Trace => 7,
    };
    let message = message.replace('\r', "\\r").replace('\n', "\\n");
    format!("<{priority}>trident-slot-loader: {message}")
}

/// Fatal errors remain visible, including when logger setup failed or logging was disabled.
pub(super) fn report_failure(error: &Error) {
    if log::log_enabled!(Level::Error) {
        error!("event=command_failed error={error:#}");
    } else {
        eprintln!(
            "{}",
            format_line(
                Level::Error,
                &format!("event=command_failed error={error:#}")
            )
        );
    }
}

/// Start records identify a stuck phase even when the unit times out before a completion record.
pub(super) fn stage<T>(
    name: &str,
    operation: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let start = Instant::now();
    info!("event=stage_start stage={name}");
    let result = operation();
    let elapsed = start.elapsed().as_millis();
    match &result {
        Ok(_) => info!("event=stage_complete stage={name} elapsed_ms={elapsed}"),
        Err(_) => error!("event=stage_failed stage={name} elapsed_ms={elapsed}"),
    }
    result.with_context(|| format!("stage={name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_records_keep_priority_and_cannot_inject_another_log_line() {
        let record = Record::builder()
            .level(Level::Error)
            .args(format_args!("event=failure\nforged"))
            .build();
        assert_eq!(
            format_record(&record),
            "<3>trident-slot-loader: event=failure\\nforged"
        );
    }

    #[test]
    fn failing_stages_preserve_the_original_error_and_add_the_phase() {
        let error = stage("mount", || -> Result<(), Error> {
            anyhow::bail!("image read failed")
        })
        .unwrap_err();
        assert_eq!(format!("{error:#}"), "stage=mount: image read failed");
    }
}
