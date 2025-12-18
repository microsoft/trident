use std::sync::{Arc, RwLock};

use anyhow::Error;
use chrono::{DateTime, Utc};
use log::{Level, Log};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default, Clone)]
pub struct LogForwarder {
    sender: Arc<RwLock<Option<UnboundedSender<ForwardedLogRecord>>>>,
}

impl LogForwarder {
    pub fn new_logger(&self) -> Box<LogForwarder> {
        Box::new(self.clone())
    }

    pub fn set_sender(&self, sender: UnboundedSender<ForwardedLogRecord>) -> Result<(), Error> {
        let mut guard = self
            .sender
            .write()
            .map_err(|_| anyhow::anyhow!("Failed to set log sender channel"))?;
        *guard = Some(sender);
        Ok(())
    }

    pub fn clear_sender(&self) -> Result<(), Error> {
        let mut guard = self
            .sender
            .write()
            .map_err(|_| anyhow::anyhow!("Failed to clear log sender channel"))?;
        *guard = None;
        Ok(())
    }

    fn has_sender(&self) -> bool {
        self.sender.read().map(|s| s.is_some()).unwrap_or_default()
    }

    fn get_sender(&self) -> Option<UnboundedSender<ForwardedLogRecord>> {
        self.sender.read().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Log for LogForwarder {
    fn enabled(&self, _: &log::Metadata) -> bool {
        self.has_sender()
    }

    fn log(&self, record: &log::Record) {
        let Some(sender) = self.get_sender() else {
            return;
        };

        let forwarded_record = ForwardedLogRecord::from(record);
        if sender.send(forwarded_record).is_err() {
            // Failed to send log record, possibly because receiver has been dropped
            // Attempt to clear the sender to avoid future attempts
            let _ = self.clear_sender();
        }
    }

    fn flush(&self) {
        todo!()
    }
}

pub struct ForwardedLogRecord {
    pub level: Level,
    pub message: String,
    pub target: String,
    pub module: String,
    pub file: String,
    pub line: u32,
    pub timestamp: DateTime<Utc>,
}

impl From<&log::Record<'_>> for ForwardedLogRecord {
    fn from(value: &log::Record) -> Self {
        Self {
            level: value.level(),
            message: value.args().to_string(),
            target: value.target().to_string(),
            module: value.module_path().unwrap_or_default().to_string(),
            file: value.file().unwrap_or_default().to_string(),
            line: value.line().unwrap_or_default(),
            timestamp: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use log::{Level, Metadata, Record};
    use tokio::sync::mpsc;

    #[test]
    fn test_log_forwarder() {
        let start_time = Utc::now();
        let log_forwarder = LogForwarder::default().new_logger();

        // Initially, no sender is set, so logging should be disabled
        assert!(!log_forwarder.enabled(&Metadata::builder().level(Level::Info).build()));

        // Set up a channel to receive forwarded log records
        let (tx, mut rx) = mpsc::unbounded_channel();
        log_forwarder.set_sender(tx).unwrap();

        // Now logging should be enabled
        assert!(log_forwarder.enabled(&Metadata::builder().level(Level::Info).build()));

        // Log a test record
        let record = Record::builder()
            .args(format_args!("Test log message"))
            .level(Level::Info)
            .target("test_target")
            .module_path(Some("test_module"))
            .file(Some("test_file.rs"))
            .line(Some(42))
            .build();
        log_forwarder.log(&record);

        // Receive the forwarded log record
        let forwarded_record = rx.blocking_recv().expect("Did not receive log record");
        let receive_time = Utc::now();

        assert_eq!(forwarded_record.level, Level::Info);
        assert_eq!(forwarded_record.message, "Test log message");
        assert_eq!(forwarded_record.target, "test_target");
        assert_eq!(forwarded_record.module, "test_module");
        assert_eq!(forwarded_record.file, "test_file.rs");
        assert_eq!(forwarded_record.line, 42);
        assert!(
            forwarded_record.timestamp >= start_time && forwarded_record.timestamp <= receive_time,
            "Timestamp is not within expected range"
        );

        drop(log_forwarder);
    }

    #[test]
    fn test_log_forwarder_clear_sender() {
        let log_forwarder = LogForwarder::default();

        // Set up a channel to receive forwarded log records
        let (tx, mut rx) = mpsc::unbounded_channel();
        log_forwarder.set_sender(tx).unwrap();

        // Clear the sender
        log_forwarder.clear_sender().unwrap();

        // Now logging should be disabled
        assert!(!log_forwarder.enabled(&Metadata::builder().level(Level::Info).build()));

        // Log a test record
        let record = Record::builder()
            .args(format_args!("Test log message after clear"))
            .level(Level::Info)
            .target("test_target")
            .module_path(Some("test_module"))
            .file(Some("test_file.rs"))
            .line(Some(42))
            .build();
        log_forwarder.log(&record);

        // There should be no forwarded log record received
        assert!(
            rx.try_recv().is_err(),
            "Received a log record when none should be sent"
        );
    }

    #[test]
    fn test_log_forwarder_closed_channel() {
        let log_forwarder = LogForwarder::default();

        // Set up a channel to receive forwarded log records
        let (tx, rx) = mpsc::unbounded_channel();
        log_forwarder.set_sender(tx).unwrap();

        assert!(log_forwarder.has_sender(), "Sender should be set");

        // Close the receiving end of the channel
        drop(rx);

        // Log a test record
        let record = Record::builder()
            .args(format_args!("Test log message after channel closed"))
            .level(Level::Info)
            .target("test_target")
            .module_path(Some("test_module"))
            .file(Some("test_file.rs"))
            .line(Some(42))
            .build();

        assert!(log_forwarder.enabled(record.metadata()));
        log_forwarder.log(&record);

        // The sender should be cleared automatically
        assert!(!log_forwarder.has_sender(), "Sender should not be set");
        assert!(!log_forwarder.enabled(&Metadata::builder().level(Level::Info).build()));

        // Nothing should happen when logging again
        log_forwarder.log(&record);
    }
}
