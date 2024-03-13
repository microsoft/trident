use std::{fs::File, io::Write, path::Path, sync::Mutex};

use log::{LevelFilter, Log, Record};

use osutils::files;

use super::LogEntry;

pub struct BackgroundLog {
    target: Option<Mutex<File>>,
    max_level: LevelFilter,
}

impl BackgroundLog {
    pub fn new(target: impl AsRef<Path>) -> Self {
        let file = match files::create_file(target.as_ref()) {
            Ok(f) => Some(Mutex::new(f)),
            Err(err) => {
                eprintln!(
                    "Logging setup error: failed to create background log file: {:?}",
                    err
                );
                None
            }
        };

        Self {
            max_level: LevelFilter::Trace,
            target: file,
        }
    }

    pub fn with_max_level(self, max_level: log::LevelFilter) -> Self {
        Self { max_level, ..self }
    }

    pub fn into_logger(self) -> Box<dyn Log> {
        Box::new(self)
    }

    /// Best effort attempt to write the log entry to the file
    fn write_entry(&self, record: &Record) -> Result<(), Box<dyn std::error::Error + '_>> {
        if let Some(file) = self.target.as_ref() {
            let mut serialized = serde_json::to_string(&LogEntry::from(record))?;
            serialized.push('\n');

            let mut file_lock = file.lock()?;
            file_lock.write_all(serialized.as_bytes())?;
            file_lock.flush()?;
        }

        Ok(())
    }
}

impl Log for BackgroundLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.target.is_some() && metadata.level() <= self.max_level
    }

    fn log(&self, record: &Record) {
        // Just try to write the log entry to the file
        let _ = self.write_entry(record);
    }

    fn flush(&self) {
        // No-op
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use log::Level;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_filter() {
        let test_dir = tempdir().unwrap();
        let target = test_dir.path().join("test.log");
        let log = BackgroundLog::new(target).with_max_level(LevelFilter::Info);
        let logger = log.into_logger();

        assert!(
            logger.enabled(&log::Metadata::builder().level(Level::Info).build()),
            "Logger should accept the record"
        );
        assert!(
            !logger.enabled(&log::Metadata::builder().level(Level::Debug).build()),
            "Logger should not accept the record"
        );
    }

    #[test]
    fn test_disabled() {
        let test_dir = tempdir().unwrap();
        // Use a directory as a target to force a failure
        let log = BackgroundLog::new(test_dir.path());
        assert!(log.target.is_none(), "Logger target should be none");
        let logger = log.into_logger();
        assert!(
            !logger.enabled(&log::Metadata::builder().level(Level::Error).build()),
            "Logger should NOT accept the record"
        );

        let log = BackgroundLog::new("/proc/readonly/fs/should/not/exist.log");
        assert!(log.target.is_none(), "Logger target should be none");
        let logger = log.into_logger();
        assert!(
            !logger.enabled(&log::Metadata::builder().level(Level::Error).build()),
            "Logger should NOT accept the record"
        );
    }

    #[test]
    fn test_background_log_file_cleanup() {
        let test_dir = tempdir().unwrap();
        let target = test_dir.path().join("test.log");

        files::write_file(&target, 0o600, "sample content".as_bytes()).unwrap();

        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("sample content"),
            "Log file should contain sample content"
        );

        // Create logger, which should clean the file
        let log = BackgroundLog::new(target.clone());
        let logger = log.into_logger();

        logger.log(
            &log::Record::builder()
                .args(format_args!("test_message"))
                .build(),
        );
        logger.flush();

        assert!(
            !fs::read_to_string(&target)
                .unwrap()
                .contains("sample content"),
            "Log file should NOT contain sample content"
        );
    }

    #[test]
    fn test_background_log() {
        let test_dir = tempdir().unwrap();
        let target = test_dir.path().join("test.log");
        let log = BackgroundLog::new(&target);
        let logger = log.into_logger();

        let record = log::Record::builder()
            .args(format_args!("test_message"))
            .level(Level::Info)
            .target("test_target")
            .module_path(Some("test_module"))
            .file(Some(file!()))
            .line(Some(42))
            .build();

        assert!(
            logger.enabled(record.metadata()),
            "Logger should accept the record"
        );

        logger.log(&record);
        logger.flush();

        let content = fs::read_to_string(target).unwrap();
        let entry: LogEntry = serde_json::from_str(&content).unwrap();
        assert_eq!(entry.level, Level::Info.into());
        assert_eq!(entry.message, "test_message");
        assert_eq!(entry.target, "test_target");
        assert_eq!(entry.module, "test_module");
        assert_eq!(entry.file, file!());
        assert_eq!(entry.line, 42);
    }
}
