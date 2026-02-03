use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};

use anyhow::{anyhow, Context, Error};
use log::{info, LevelFilter, Log, Metadata, Record};
use url::Url;

use super::{background_uploader::BackgroundUploadHandle, filter::LogFilter, LogEntry};

type Remote = Arc<RwLock<Option<Url>>>;

#[derive(Clone)]
pub struct Logstream {
    // TODO: Consider changing this to a LockOnce when rustc is updated to >=1.70
    target: Remote,
    disabled: bool,
    uploader: BackgroundUploadHandle,
}

impl Logstream {
    pub fn create(uploader: BackgroundUploadHandle) -> Self {
        Self {
            target: Arc::new(RwLock::new(None)),
            disabled: false,
            uploader,
        }
    }

    /// Permanently disable the logstream
    ///
    /// Useful for cases when we know we don't want to send logs to the server
    pub fn disable(&mut self) {
        self.disabled = true;
    }

    /// Set the logstream server URL
    ///
    /// If the logstream is disabled, this is a no-op.
    pub fn set_server(&self, url: String) -> Result<(), Error> {
        if self.disabled {
            info!("Logstream is disabled, ignoring set_server");
            return Ok(());
        }

        let url = Url::parse(&url).context("Failed to parse logstream URL")?;
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow!("Failed to lock logstream"))?;
        val.replace(url);
        Ok(())
    }

    /// Clear the logstream server URL
    ///
    /// This will stop logs from being sent to the server.
    pub fn clear_server(&self) -> Result<(), Error> {
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow!("Failed to lock logstream"))?;
        val.take();
        Ok(())
    }

    /// Create a Boxed LogSender
    ///
    /// Sets the max level to Debug
    pub fn make_logger(&self) -> Box<LogFilter<LogSender>> {
        self.make_logger_inner(LevelFilter::Debug)
    }

    /// Create a Boxed LogSender with a specific max level
    pub fn make_logger_with_level(&self, max_level: LevelFilter) -> Box<LogFilter<LogSender>> {
        self.make_logger_inner(max_level)
    }

    /// Internal function to create the logger with a specific max level and
    /// filters to avoid recursion.
    fn make_logger_inner(&self, max_level: LevelFilter) -> Box<LogFilter<LogSender>> {
        LogFilter::new(LogSender::new(
            self.target.clone(),
            max_level,
            self.uploader.clone(),
        ))
        // Filter all logs that could be produced as part of sending logs to avoid recursion.
        .with_global_filter("hyper", LevelFilter::Error)
        .with_global_filter("hyper_util", LevelFilter::Error)
        .with_global_filter("request", LevelFilter::Error)
        .with_global_filter(module_path!(), LevelFilter::Error)
        .into_logger()
    }
}

/// A logger that sends logs to a server
///
/// This logger is designed to be used with a MultiLogger.
///
/// Do not create this logger directly, use Logstream::make_logger instead.
pub struct LogSender {
    max_level: LevelFilter,
    server: Remote,
    uploader: BackgroundUploadHandle,
    send_failed: AtomicBool,
}

impl LogSender {
    fn new(server: Remote, max_level: LevelFilter, uploader: BackgroundUploadHandle) -> Self {
        Self {
            server,
            max_level,
            uploader,
            send_failed: AtomicBool::new(false),
        }
    }

    pub fn with_max_level(self, max_level: LevelFilter) -> Self {
        Self { max_level, ..self }
    }

    fn has_server(&self) -> bool {
        self.server.read().map(|s| s.is_some()).unwrap_or_default()
    }

    fn get_server(&self) -> Option<Url> {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Log for LogSender {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Block logs with a level higher than the max level
        // Block reqwest logs from being sent to the server
        // Block logs if there is no server
        // Blocks logs from request to avoid logging recursively
        metadata.level() <= self.max_level
            && !metadata.target().starts_with("reqwest")
            && self.has_server()
    }

    fn log(&self, record: &Record) {
        if let Some(target) = self.get_server() {
            let body = match serde_json::to_string(&LogEntry::from(record)) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Failed to serialize log entry: {e}");
                    return;
                }
            };

            if let Err(e) = self.uploader.upload(&target, body) {
                if !self.send_failed.swap(true, Ordering::Relaxed) {
                    eprintln!("Failed to send log entry: {e}");
                }
            }
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logstream() {
        let logstream = Logstream::create(BackgroundUploadHandle::new_mock());
        let logger = logstream.make_logger().into_inner();

        assert!(!logger.has_server(), "Logstream should not have a server");
        assert!(
            logger.get_server().is_none(),
            "Logstream should not have a server"
        );

        logstream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        assert!(logger.has_server(), "Logstream should have a server");
        assert_eq!(
            logger.get_server().unwrap(),
            Url::parse("http://localhost:8080").unwrap(),
            "Logstream should have a server"
        );

        assert!(
            logger.enabled(&log::Metadata::builder().level(log::Level::Error).build()),
            "Logstream should be enabled"
        );
    }

    #[test]
    fn test_lock() {
        let mut logstream = Logstream::create(BackgroundUploadHandle::new_mock());
        let logger = logstream.make_logger().into_inner();

        assert!(!logger.has_server(), "Logstream should not have a server");
        assert!(
            logger.get_server().is_none(),
            "Logstream should not have a server"
        );

        logstream.disable();

        logstream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        assert!(!logger.has_server(), "Logstream should not have a server");
        assert!(
            logger.get_server().is_none(),
            "Logstream should not have a server"
        );

        assert!(
            !logger.enabled(&log::Metadata::builder().level(log::Level::Error).build()),
            "Logstream should be disabled"
        );
    }
}
