//! Persists installer diagnostics and mirrors eligible records to serial consoles.

use std::{fs::File, io::Write, path::PathBuf};

use anyhow::{Context, Error};
use log::{warn, LevelFilter};

use crate::client::Event;

use super::model::{log_visible, LogEntry};

pub(crate) struct Output {
    pub(crate) log: Option<File>,
    pub(crate) mirrors: Vec<(PathBuf, File)>,
}

impl Output {
    /// Persists the complete event in the diagnostic file, if one is configured.
    pub(super) fn record(&mut self, event: &Event) -> Result<(), Error> {
        if let Some(log) = &mut self.log {
            writeln!(log, "{event:#?}").context("Failed to preserve installer diagnostics")?;
            log.flush()?;
        }
        Ok(())
    }

    /// Sends visible records to serial mirrors, dropping mirrors that fail to write.
    pub(super) fn announce(&mut self, entry: &LogEntry, verbosity: LevelFilter) {
        if !log_visible(entry.level, verbosity) {
            return;
        }
        self.mirrors.retain_mut(|(path, mirror)| {
            if let Err(error) = writeln!(mirror, "{}", entry.plain()) {
                warn!(
                    "Stopped mirroring installer status to '{}': {error}",
                    path.display()
                );
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::{self, OpenOptions},
        time::Duration,
    };

    use tempfile::TempDir;
    use trident_client::LogLevel;

    use crate::config::SerialVerbosity;

    use super::super::model::{LogSource, Model};

    /// A failed serial destination is removed without blocking healthy mirrors.
    #[test]
    fn unavailable_serial_mirror_does_not_stop_the_installer() {
        let root = TempDir::new().unwrap();
        let transcript = root.path().join("serial.log");
        let mut output = Output {
            log: None,
            mirrors: vec![
                (
                    PathBuf::from("/dev/full"),
                    OpenOptions::new().write(true).open("/dev/full").unwrap(),
                ),
                (
                    transcript.clone(),
                    OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .open(&transcript)
                        .unwrap(),
                ),
            ],
        };
        let entry = LogEntry {
            elapsed: Duration::from_secs(125),
            source: LogSource::Inst,
            level: LogLevel::Info,
            message: "Preparing installer".into(),
            result_color: None,
        };
        output.announce(&entry, LevelFilter::Debug);
        assert_eq!(output.mirrors.len(), 1);
        output.announce(
            &LogEntry {
                message: "Still running".into(),
                ..entry
            },
            LevelFilter::Debug,
        );
        assert_eq!(
            fs::read_to_string(transcript).unwrap(),
            "02:05 [INST:INFO] Preparing installer\n02:05 [INST:INFO] Still running\n"
        );
    }

    /// Serial filtering preserves line prefixes and does not change display filtering.
    #[test]
    fn serial_verbosity_filters_independently_of_display() {
        let mut model = Model::new();
        let root = TempDir::new().unwrap();
        let serial = root.path().join("serial.log");
        let mut output = Output {
            log: None,
            mirrors: vec![(serial.clone(), File::create(&serial).unwrap())],
        };
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
        assert!(!log_visible(LogLevel::Trace, model.verbosity));
        assert_eq!(model.logs.len(), 6);
    }
}
