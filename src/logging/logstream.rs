use std::sync::{Arc, RwLock};

use anyhow::Context;
use log::{info, Log};

use super::LogEntry;

pub struct Logstream {
    // TODO: Consider changing this to a LockOnce when rustc is updated to >=1.70
    target: Arc<RwLock<Option<String>>>,
    disabled: bool,
}

impl Logstream {
    pub fn create() -> Self {
        Self {
            target: Arc::new(RwLock::new(None)),
            disabled: false,
        }
    }

    /// Permanently disable the logstream
    ///
    /// Useful for cases when we know we don't want to send logs to the server
    pub fn disable(&mut self) {
        self.disabled = true;
    }

    pub fn set_server(&self, url: String) -> Result<(), anyhow::Error> {
        if self.disabled {
            info!("Logstream is disabled, ignoring set_server");
            return Ok(());
        }

        reqwest::Url::parse(&url).context("Failed to parse logstream URL")?;
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow::anyhow!("Failed to lock logstream"))?;
        val.replace(url);
        Ok(())
    }

    /// Create a Boxed LogSender
    pub fn make_logger(&self) -> Box<LogSender> {
        Box::new(LogSender::new(self.target.clone()))
    }
}

/// A logger that sends logs to a server
///
/// This logger is designed to be used with a MultiLogger.
///
/// Do not create this logger directly, use Logstream::make_logger instead.
pub struct LogSender {
    max_level: log::LevelFilter,
    server: Arc<RwLock<Option<String>>>,
    client: reqwest::blocking::Client,
}

impl LogSender {
    fn new(server: Arc<RwLock<Option<String>>>) -> Self {
        Self {
            server,
            max_level: log::LevelFilter::Debug,
            client: reqwest::blocking::Client::new(),
        }
    }

    pub fn with_max_level(self, max_level: log::LevelFilter) -> Self {
        Self { max_level, ..self }
    }

    fn has_server(&self) -> bool {
        self.server.read().map(|s| s.is_some()).unwrap_or_default()
    }

    fn get_server(&self) -> Option<String> {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Log for LogSender {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // BLock logs with a level higher than the max level
        // Block reqwest logs from being sent to the server
        // Block logs if there is no server
        metadata.level() <= self.max_level
            && !metadata.target().starts_with("reqwest")
            && self.has_server()
    }

    fn log(&self, record: &log::Record) {
        if let Some(target) = self.get_server() {
            let body = match serde_json::to_string(&LogEntry::from(record)) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Failed to serialize log entry: {}", e);
                    return;
                }
            };

            if let Err(e) = self.client.post(target).body(body).send() {
                eprintln!("Failed to send log entry: {}", e);
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
        let logstream = Logstream::create();
        let logger = logstream.make_logger();

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
            "http://localhost:8080",
            "Logstream should have a server"
        );

        assert!(
            logger.enabled(&log::Metadata::builder().level(log::Level::Error).build()),
            "Logstream should be enabled"
        );
    }

    #[test]
    fn test_lock() {
        let mut logstream = Logstream::create();
        let logger = logstream.make_logger();

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
