use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, RwLock,
};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Error};
use log::{info, Log};

use super::LogEntry;

#[derive(Clone)]
pub struct LogstreamAsync {
    // TODO: Consider changing this to a LockOnce when rustc is updated to >=1.70
    target: Arc<RwLock<Option<String>>>,
    disabled: bool,
}

impl LogstreamAsync {
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

    /// Set the logstream server URL
    ///
    /// If the logstream is disabled, this is a no-op.
    pub fn set_server(&self, url: String) -> Result<(), Error> {
        if self.disabled {
            info!("Logstream is disabled, ignoring set_server");
            return Ok(());
        }

        reqwest::Url::parse(&url).context("Failed to parse logstream URL")?;
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

    /// Create a Boxed AsyncLogSender
    ///
    /// Sets the max level to Debug
    pub fn make_logger(&self) -> Box<AsyncLogSender> {
        Box::new(AsyncLogSender::new(self.target.clone(), log::LevelFilter::Debug))
    }

    /// Create a Boxed AsyncLogSender with a specific max level
    pub fn make_logger_with_level(&self, max_level: log::LevelFilter) -> Box<AsyncLogSender> {
        Box::new(AsyncLogSender::new(self.target.clone(), max_level))
    }
}

/// Message sent through the channel to the worker thread
struct LogMessage {
    entry: LogEntry,
    target_url: String,
}

/// A logger that sends logs to a server asynchronously via a sidecar thread
///
/// This logger spawns a background thread that handles all HTTP requests,
/// allowing the main thread to continue without blocking on network I/O.
///
/// Do not create this logger directly, use LogstreamAsync::make_logger instead.
pub struct AsyncLogSender {
    max_level: log::LevelFilter,
    server: Arc<RwLock<Option<String>>>,
    sender: Option<Sender<LogMessage>>,
    worker_thread: Option<JoinHandle<()>>,
    send_failed: Arc<AtomicBool>,
}

impl AsyncLogSender {
    fn new(server: Arc<RwLock<Option<String>>>, max_level: log::LevelFilter) -> Self {
        let (sender, receiver) = mpsc::channel();
        let send_failed = Arc::new(AtomicBool::new(false));
        let send_failed_clone = send_failed.clone();

        // Spawn the worker thread that will send logs to the server
        let worker_thread = thread::spawn(move || {
            Self::worker_loop(receiver, send_failed_clone);
        });

        Self {
            server,
            max_level,
            sender: Some(sender),
            worker_thread: Some(worker_thread),
            send_failed,
        }
    }

    /// Worker thread loop that processes log entries from the channel
    fn worker_loop(receiver: Receiver<LogMessage>, send_failed: Arc<AtomicBool>) {
        let client = reqwest::blocking::Client::new();

        // Process all log entries until the channel is closed
        while let Ok(log_message) = receiver.recv() {
            // Serialize the log entry
            let body = match serde_json::to_string(&log_message.entry) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Failed to serialize log entry: {e}");
                    continue;
                }
            };

            // Send the log to the server
            if let Err(e) = client.post(&log_message.target_url).body(body).send() {
                if !send_failed.swap(true, Ordering::Relaxed) {
                    eprintln!("Failed to send log entry: {e}");
                }
            }
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

    /// Finish sending all pending logs and shut down the worker thread
    ///
    /// This method should be called when no more logs will be sent.
    /// It drops the sender channel (signaling completion) and waits for
    /// the worker thread to finish processing all queued logs.
    pub fn finish(&mut self) {
        // Drop the sender to signal the worker thread to finish
        self.sender.take();

        // Wait for the worker thread to complete
        if let Some(handle) = self.worker_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Log for AsyncLogSender {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Block logs with a level higher than the max level
        // Block reqwest logs from being sent to the server
        // Block logs if there is no server
        // Blocks logs from request to avoid logging recursively
        metadata.level() <= self.max_level
            && !metadata.target().starts_with("reqwest")
            && self.has_server()
    }

    fn log(&self, record: &log::Record) {
        if let Some(target) = self.get_server() {
            let log_entry = LogEntry::from(record);
            
            // Try to send the log entry to the worker thread
            if let Some(sender) = &self.sender {
                let message = LogMessage {
                    entry: log_entry,
                    target_url: target,
                };
                
                // Send is non-blocking on the sender side
                // If the channel is full or closed, we just drop the log
                let _ = sender.send(message);
            }
        }
    }

    fn flush(&self) {
        // The worker thread continuously processes logs
        // Flush is essentially a no-op in this async design
    }
}

impl Drop for AsyncLogSender {
    fn drop(&mut self) {
        // Ensure the worker thread is properly shut down
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_logstream_async() {
        let logstream = LogstreamAsync::create();
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
        let mut logstream = LogstreamAsync::create();
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

    #[test]
    fn test_finish_method() {
        let logstream = LogstreamAsync::create();
        logstream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        let mut logger = logstream.make_logger();

        // Log some messages
        let record = log::Record::builder()
            .args(format_args!("test message"))
            .level(log::Level::Info)
            .build();

        logger.log(&record);
        logger.log(&record);
        logger.log(&record);

        // Finish should wait for all logs to be processed
        logger.finish();

        // After finish, the sender should be None
        assert!(logger.sender.is_none(), "Sender should be None after finish");
        assert!(
            logger.worker_thread.is_none(),
            "Worker thread should be None after finish"
        );
    }

    #[test]
    fn test_drop_cleanup() {
        let logstream = LogstreamAsync::create();
        logstream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        {
            let logger = logstream.make_logger();

            // Log some messages
            let record = log::Record::builder()
                .args(format_args!("test message"))
                .level(log::Level::Info)
                .build();

            logger.log(&record);
            // Logger is dropped here, should clean up properly
        }

        // If we get here without hanging, the drop worked correctly
    }

    #[test]
    fn test_channel_communication() {
        let logstream = LogstreamAsync::create();
        logstream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        let mut logger = logstream.make_logger();

        // Create several log records
        for i in 0..10 {
            let record = log::Record::builder()
                .args(format_args!("test message {}", i))
                .level(log::Level::Info)
                .build();
            logger.log(&record);
        }

        // Give the worker thread a moment to process
        std::thread::sleep(Duration::from_millis(100));

        // Finish and ensure cleanup
        logger.finish();
    }
}
