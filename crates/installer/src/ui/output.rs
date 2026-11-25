//! Persists installer diagnostics and mirrors eligible records to serial consoles.

use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{warn, LevelFilter};

use crate::client::Event;

use super::model::{log_visible, LogEntry};

/// Bounds memory use when a serial consumer remains backpressured.
const SERIAL_BACKLOG_LIMIT_BYTES: usize = 64 * 1024;

/// One nonblocking serial destination and its unwritten output.
pub(crate) struct SerialMirror {
    /// Device path used for diagnostics.
    path: PathBuf,
    /// Nonblocking console handle.
    file: File,
    /// Bytes retained across partial writes and temporary backpressure.
    pending: VecDeque<u8>,
}

impl SerialMirror {
    /// Creates a mirror with no pending output.
    pub(crate) fn new(path: impl AsRef<Path>, file: File) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            file,
            pending: VecDeque::new(),
        }
    }

    /// Queues one complete status record, bounding stale output during sustained backpressure.
    fn enqueue(&mut self, data: &[u8]) -> EnqueueOutcome {
        if data.len() > SERIAL_BACKLOG_LIMIT_BYTES {
            return EnqueueOutcome::RecordTooLarge(data.len());
        }
        let dropped = (self.pending.len().saturating_add(data.len()) > SERIAL_BACKLOG_LIMIT_BYTES)
            .then(|| {
                let bytes = self.pending.len();
                self.pending.clear();
                bytes
            });
        self.pending.extend(data);
        dropped.map_or(EnqueueOutcome::Queued, EnqueueOutcome::DroppedBacklog)
    }

    /// Writes as much queued output as the nonblocking destination currently accepts.
    fn flush_pending(&mut self) -> io::Result<FlushOutcome> {
        while !self.pending.is_empty() {
            let (front, back) = self.pending.as_slices();
            let data = if front.is_empty() { back } else { front };
            match self.file.write(data) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "serial mirror accepted zero bytes",
                    ));
                }
                Ok(written) => {
                    self.pending.drain(..written);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(FlushOutcome::Backpressured);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(FlushOutcome::Drained)
    }
}

/// Result of adding a record to a serial mirror's bounded queue.
enum EnqueueOutcome {
    /// The record was queued without discarding older output.
    Queued,
    /// Older queued bytes were discarded so the newest record could be retained.
    DroppedBacklog(usize),
    /// A single record exceeded the complete queue capacity and was skipped.
    RecordTooLarge(usize),
}

/// Result of attempting to flush a nonblocking serial mirror.
enum FlushOutcome {
    /// Every queued byte was written.
    Drained,
    /// Unwritten bytes remain queued after `EAGAIN`.
    Backpressured,
}

/// Writes complete diagnostics privately and mirrors filtered status to serial ports.
pub(crate) struct Output {
    /// Opened by startup with restrictive permissions; optional in tests.
    pub(crate) log: Option<File>,
    /// Serial destinations with independent bounded pending output.
    pub(crate) mirrors: Vec<SerialMirror>,
}

impl Output {
    /// Preserve the event before the UI shortens its display copy.
    pub(super) fn record(&mut self, event: &Event) -> Result<(), Error> {
        if let Some(log) = &mut self.log {
            writeln!(log, "{event:#?}").context("Failed to preserve installer diagnostics")?;
            log.flush()?;
        }
        Ok(())
    }

    /// Sends visible records without dropping mirrors for temporary write backpressure.
    pub(super) fn announce(&mut self, entry: &LogEntry, verbosity: LevelFilter) {
        if !log_visible(entry.level, verbosity) {
            return;
        }
        let mut record = entry.plain().into_bytes();
        record.push(b'\n');
        self.mirrors.retain_mut(|mirror| {
            match mirror.enqueue(&record) {
                EnqueueOutcome::Queued => {}
                EnqueueOutcome::DroppedBacklog(bytes) => warn!(
                    "Discarded {bytes} queued serial bytes for '{}' after sustained backpressure",
                    mirror.path.display()
                ),
                EnqueueOutcome::RecordTooLarge(bytes) => {
                    warn!(
                        "Skipped {bytes}-byte installer status record for '{}'; serial backlog limit is {SERIAL_BACKLOG_LIMIT_BYTES} bytes",
                        mirror.path.display()
                    );
                    return true;
                }
            }
            true
        });
        self.flush_mirrors();
    }

    /// Retries queued serial output, removing only mirrors with permanent write failures.
    pub(super) fn flush_mirrors(&mut self) {
        self.mirrors
            .retain_mut(|mirror| match mirror.flush_pending() {
                Ok(FlushOutcome::Drained | FlushOutcome::Backpressured) => true,
                Err(error) => {
                    warn!(
                        "Stopped mirroring installer status to '{}': {error}",
                        mirror.path.display()
                    );
                    false
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::{self, OpenOptions},
        io::{self, Read, Write},
        time::Duration,
    };

    use nix::{fcntl::OFlag, unistd};
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
                SerialMirror::new(
                    PathBuf::from("/dev/full"),
                    OpenOptions::new().write(true).open("/dev/full").unwrap(),
                ),
                SerialMirror::new(
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

    /// Temporary serial backpressure must not permanently remove the mirror.
    #[test]
    fn temporary_serial_backpressure_keeps_mirror() {
        let (reader, writer) = unistd::pipe2(OFlag::O_NONBLOCK).unwrap();
        let mut reader = File::from(reader);
        let mut mirror = File::from(writer);
        let data = [0_u8; 4096];
        let mut backpressured = false;
        for _ in 0..4096 {
            match mirror.write(&data) {
                Ok(0) => panic!("Serial mirror stopped accepting data without EAGAIN"),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    backpressured = true;
                    break;
                }
                Err(error) => panic!("Failed to fill serial mirror: {error}"),
            }
        }
        assert!(backpressured, "Serial mirror never returned EAGAIN");

        let mut output = Output {
            log: None,
            mirrors: vec![SerialMirror::new("/dev/ttyS0", mirror)],
        };
        let first = LogEntry {
            elapsed: Duration::ZERO,
            source: LogSource::Inst,
            level: LogLevel::Info,
            message: "Temporary backpressure".into(),
            result_color: None,
        };
        output.announce(&first, LevelFilter::Debug);
        assert_eq!(output.mirrors.len(), 1);
        assert!(!output.mirrors[0].pending.is_empty());

        let mut drained = [0_u8; 4096];
        loop {
            match reader.read(&mut drained) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("Failed to drain serial mirror: {error}"),
            }
        }
        output.flush_mirrors();
        assert!(output.mirrors[0].pending.is_empty());

        let mut written = Vec::new();
        match reader.read_to_end(&mut written) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("Failed to read recovered serial output: {error}"),
        }
        assert_eq!(
            String::from_utf8(written.clone()).unwrap(),
            "00:00 [INST:INFO] Temporary backpressure\n"
        );

        let second = LogEntry {
            message: "Mirror recovered".into(),
            ..first
        };
        output.announce(&second, LevelFilter::Debug);
        assert!(output.mirrors[0].pending.is_empty());

        match reader.read_to_end(&mut written) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("Failed to read recovered serial output: {error}"),
        }
        assert_eq!(
            String::from_utf8(written).unwrap(),
            "00:00 [INST:INFO] Temporary backpressure\n00:00 [INST:INFO] Mirror recovered\n"
        );
    }

    /// Sustained backpressure retains the newest record without unbounded memory growth.
    #[test]
    fn serial_backlog_is_bounded_and_keeps_latest_record() {
        let (_reader, writer) = unistd::pipe2(OFlag::O_NONBLOCK).unwrap();
        let mut mirror = SerialMirror::new("/dev/ttyS0", File::from(writer));
        mirror
            .pending
            .extend(vec![1_u8; SERIAL_BACKLOG_LIMIT_BYTES - 4]);

        assert!(matches!(
            mirror.enqueue(&[2_u8; 8]),
            EnqueueOutcome::DroppedBacklog(bytes) if bytes == SERIAL_BACKLOG_LIMIT_BYTES - 4
        ));
        assert_eq!(mirror.pending, [2_u8; 8]);

        assert!(matches!(
            mirror.enqueue(&vec![3_u8; SERIAL_BACKLOG_LIMIT_BYTES + 1]),
            EnqueueOutcome::RecordTooLarge(bytes) if bytes == SERIAL_BACKLOG_LIMIT_BYTES + 1
        ));
        assert_eq!(mirror.pending, [2_u8; 8]);
    }

    /// Serial filtering preserves line prefixes and does not change display filtering.
    #[test]
    fn serial_verbosity_filters_independently_of_display() {
        let mut model = Model::new();
        let root = TempDir::new().unwrap();
        let serial = root.path().join("serial.log");
        let mut output = Output {
            log: None,
            mirrors: vec![SerialMirror::new(&serial, File::create(&serial).unwrap())],
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
