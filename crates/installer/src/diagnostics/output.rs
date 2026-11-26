//! Persists installer diagnostics and mirrors eligible records to serial consoles.

use std::{fs::File, io::Write};

use anyhow::{Context, Error};
use log::LevelFilter;

use crate::{client::Event, serial::SerialMirrors};

use super::{log_visible, Record};

/// Writes complete diagnostics privately and mirrors filtered status to serial ports.
pub(crate) struct Output {
    /// Opened by startup with restrictive permissions; optional in tests.
    pub(crate) log: Option<File>,
    /// Serial destinations with independent bounded pending output.
    pub(crate) mirrors: SerialMirrors,
}

impl Output {
    /// Persist the complete, already-redacted event before a frontend shortens its display copy.
    pub(crate) fn record(&mut self, event: &Event) -> Result<(), Error> {
        if let Some(log) = &mut self.log {
            writeln!(log, "{event:#?}").context("Failed to preserve installer diagnostics")?;
            log.flush()?;
        }
        Ok(())
    }

    /// Sends visible records without dropping mirrors for temporary write backpressure.
    pub(crate) fn announce(&mut self, entry: &Record, verbosity: LevelFilter) {
        if !log_visible(entry.level, verbosity) {
            return;
        }
        let mut record = entry.plain().into_bytes();
        record.push(b'\n');
        self.mirrors.announce(&record);
    }

    /// Retries queued serial output, removing only mirrors with permanent write failures.
    pub(crate) fn flush_mirrors(&mut self) {
        self.mirrors.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;
    use trident_client::LogLevel;

    use crate::config::SerialVerbosity;

    use crate::{diagnostics::Source as LogSource, engine::State};

    /// Serial filtering preserves line prefixes and does not change display filtering.
    #[test]
    fn serial_verbosity_filters_independently_of_display() {
        let model = State::new();
        let root = TempDir::new().unwrap();
        let serial = root.path().join("serial.log");
        let mut mirrors = SerialMirrors::default();
        mirrors.push(&serial, File::create(&serial).unwrap());
        let mut output = Output { log: None, mirrors };
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let entry = model.log(LogSource::Trident, level, "first\nsecond".into());
            output.announce(&entry, SerialVerbosity::Debug.filter());
        }
        let text = fs::read_to_string(&serial).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().all(|line| line.starts_with("00:00 [TRIDENT:")));
        assert_eq!(lines[0], "00:00 [TRIDENT:ERROR] first");
        assert_eq!(lines[1], "00:00 [TRIDENT:ERROR] second");
        assert!(!text.contains("\\n"));
        let trace = model.log(LogSource::Trident, LogLevel::Trace, "trace detail".into());
        output.announce(&trace, SerialVerbosity::Trace.filter());
        let text = fs::read_to_string(serial).unwrap();
        assert!(text
            .lines()
            .last()
            .unwrap()
            .ends_with("[TRIDENT:TRACE] trace detail"));
    }
}
