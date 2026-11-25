//! Mirrors installer status to nonblocking serial consoles.

use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

use log::warn;

/// Bounds memory use when a serial consumer remains backpressured.
const SERIAL_BACKLOG_LIMIT_BYTES: usize = 64 * 1024;
/// Leave room to terminate a partially written line when stale output is discarded.
const SERIAL_RECORD_LIMIT_BYTES: usize = SERIAL_BACKLOG_LIMIT_BYTES - 1;

/// Serial destinations with independent bounded pending output.
#[derive(Default)]
pub(crate) struct SerialMirrors {
    mirrors: Vec<SerialMirror>,
}

impl SerialMirrors {
    /// Adds one nonblocking serial destination.
    pub(crate) fn push(&mut self, path: impl AsRef<Path>, file: File) {
        self.mirrors.push(SerialMirror::new(path, file));
    }

    /// Queues one complete record and writes as much as each destination currently accepts.
    pub(crate) fn announce(&mut self, record: &[u8]) {
        self.mirrors.retain_mut(|mirror| {
            match mirror.enqueue(record) {
                EnqueueOutcome::Queued => {}
                EnqueueOutcome::DroppedBacklog(bytes) => warn!(
                    "Discarded {bytes} queued serial bytes for '{}' after sustained backpressure",
                    mirror.path.display()
                ),
                EnqueueOutcome::RecordTooLarge(bytes) => {
                    warn!(
                        "Skipped {bytes}-byte installer status record for '{}'; serial record limit is {SERIAL_RECORD_LIMIT_BYTES} bytes",
                        mirror.path.display()
                    );
                    return true;
                }
            }
            true
        });
        self.flush();
    }

    /// Retries pending output, removing only destinations with permanent write failures.
    pub(crate) fn flush(&mut self) {
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

/// One nonblocking serial destination and its unwritten output.
struct SerialMirror {
    path: PathBuf,
    file: File,
    pending: VecDeque<u8>,
}

impl SerialMirror {
    /// Creates a mirror with no pending output.
    fn new(path: impl AsRef<Path>, file: File) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            file,
            pending: VecDeque::new(),
        }
    }

    /// Queues one complete record, bounding stale output during sustained backpressure.
    /// An overflow keeps a separator so the next record cannot join an already-written fragment.
    fn enqueue(&mut self, data: &[u8]) -> EnqueueOutcome {
        if data.len() > SERIAL_RECORD_LIMIT_BYTES {
            return EnqueueOutcome::RecordTooLarge(data.len());
        }
        let dropped = (self.pending.len().saturating_add(data.len()) > SERIAL_BACKLOG_LIMIT_BYTES)
            .then(|| {
                let bytes = self.pending.len();
                self.pending.clear();
                // The old record may already be partly on the wire, without its final newline.
                self.pending.push_back(b'\n');
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
    Queued,
    DroppedBacklog(usize),
    RecordTooLarge(usize),
}

/// Result of attempting to flush a nonblocking serial mirror.
enum FlushOutcome {
    Drained,
    Backpressured,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::{self, OpenOptions},
        io::{Read, Write},
    };

    use nix::{
        fcntl::{self, FcntlArg, OFlag},
        unistd,
    };
    use tempfile::TempDir;

    #[test]
    fn permanent_failure_does_not_stop_healthy_mirror() {
        let root = TempDir::new().unwrap();
        let transcript = root.path().join("serial.log");
        let mut mirrors = SerialMirrors::default();
        mirrors.push(
            "/dev/full",
            OpenOptions::new().write(true).open("/dev/full").unwrap(),
        );
        mirrors.push(&transcript, File::create(&transcript).unwrap());

        mirrors.announce(b"first\n");
        assert_eq!(mirrors.mirrors.len(), 1);
        mirrors.announce(b"second\n");
        assert_eq!(fs::read_to_string(transcript).unwrap(), "first\nsecond\n");
    }

    #[test]
    fn temporary_backpressure_keeps_mirror() {
        let (reader, writer) = unistd::pipe2(OFlag::O_NONBLOCK).unwrap();
        let mut reader = File::from(reader);
        let mut writer = File::from(writer);
        let data = [0_u8; 4096];
        let mut backpressured = false;
        for _ in 0..4096 {
            match writer.write(&data) {
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

        let mut mirrors = SerialMirrors::default();
        mirrors.push("/dev/ttyS0", writer);
        mirrors.announce(b"Temporary backpressure\n");
        assert_eq!(mirrors.mirrors.len(), 1);
        assert!(!mirrors.mirrors[0].pending.is_empty());

        let mut drained = [0_u8; 4096];
        loop {
            match reader.read(&mut drained) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("Failed to drain serial mirror: {error}"),
            }
        }
        mirrors.flush();
        assert!(mirrors.mirrors[0].pending.is_empty());

        let mut written = Vec::new();
        match reader.read_to_end(&mut written) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("Failed to read recovered serial output: {error}"),
        }
        assert_eq!(written, b"Temporary backpressure\n");

        mirrors.announce(b"Mirror recovered\n");
        assert!(mirrors.mirrors[0].pending.is_empty());
        match reader.read_to_end(&mut written) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("Failed to read recovered serial output: {error}"),
        }
        assert_eq!(written, b"Temporary backpressure\nMirror recovered\n");
    }

    #[test]
    fn backlog_is_bounded_and_keeps_latest_record() {
        let (_reader, writer) = unistd::pipe2(OFlag::O_NONBLOCK).unwrap();
        let mut mirror = SerialMirror::new("/dev/ttyS0", File::from(writer));
        mirror
            .pending
            .extend(vec![1_u8; SERIAL_BACKLOG_LIMIT_BYTES - 4]);

        assert!(matches!(
            mirror.enqueue(&[2_u8; 8]),
            EnqueueOutcome::DroppedBacklog(bytes) if bytes == SERIAL_BACKLOG_LIMIT_BYTES - 4
        ));
        let expected = [b"\n".as_slice(), &[2_u8; 8]].concat();
        assert_eq!(mirror.pending, expected);

        assert!(matches!(
            mirror.enqueue(&vec![3_u8; SERIAL_BACKLOG_LIMIT_BYTES]),
            EnqueueOutcome::RecordTooLarge(bytes) if bytes == SERIAL_BACKLOG_LIMIT_BYTES
        ));
        assert_eq!(mirror.pending, expected);
    }

    /// Dropping an unfinished record must not join its written prefix to the newest record.
    #[test]
    fn partial_write_then_overflow_preserves_record_boundary() {
        const PIPE_CAPACITY_BYTES: i32 = 4096;

        let (reader, writer) = unistd::pipe2(OFlag::O_NONBLOCK).unwrap();
        fcntl::fcntl(&writer, FcntlArg::F_SETPIPE_SZ(PIPE_CAPACITY_BYTES)).unwrap();
        let mut reader = File::from(reader);
        let mut mirror = SerialMirror::new("/dev/ttyS0", File::from(writer));
        let mut original = vec![b'x'; SERIAL_RECORD_LIMIT_BYTES];
        *original.last_mut().unwrap() = b'\n';
        assert!(matches!(mirror.enqueue(&original), EnqueueOutcome::Queued));
        assert!(matches!(
            mirror.flush_pending().unwrap(),
            FlushOutcome::Backpressured
        ));
        let written_prefix = original.len() - mirror.pending.len();
        assert!(written_prefix > 0 && written_prefix < original.len());

        // Fill the entire budget with the replacement and its required separator.
        let mut latest = vec![b'n'; SERIAL_RECORD_LIMIT_BYTES];
        *latest.last_mut().unwrap() = b'\n';
        assert!(matches!(
            mirror.enqueue(&latest),
            EnqueueOutcome::DroppedBacklog(_)
        ));
        assert_eq!(mirror.pending.len(), SERIAL_BACKLOG_LIMIT_BYTES);
        assert!(matches!(
            mirror.flush_pending().unwrap(),
            FlushOutcome::Backpressured
        ));

        let mut output = Vec::new();
        loop {
            match reader.read_to_end(&mut output) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("Failed to read serial output: {error}"),
            }
            if mirror.pending.is_empty() {
                break;
            }
            mirror.flush_pending().unwrap();
        }
        assert_eq!(&output[..written_prefix], &original[..written_prefix]);
        assert_eq!(output[written_prefix], b'\n');
        assert_eq!(&output[written_prefix + 1..], latest);
    }
}
