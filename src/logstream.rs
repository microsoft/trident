use std::sync::{Arc, RwLock};

use anyhow::Context;
use log::Log;
use serde::Serialize;

pub struct Logstream {
    // TODO: Consider changing this to a LockOnce when rustc is updated to >=1.70
    target: Arc<RwLock<Option<String>>>,
}

impl Logstream {
    pub fn create() -> Self {
        Self {
            target: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_server(&self, url: String) -> Result<(), anyhow::Error> {
        reqwest::Url::parse(&url).context("Failed to parse logstream URL")?;
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow::anyhow!("Failed to lock logstream"))?;
        val.replace(url);
        Ok(())
    }

    pub fn make_logger(&self) -> Box<LogSender> {
        Box::new(LogSender::new(self.target.clone()))
    }
}

pub struct LogSender {
    max_level: log::LevelFilter,
    server: Arc<RwLock<Option<String>>>,
    client: reqwest::blocking::Client,
}

impl LogSender {
    pub fn new(server: Arc<RwLock<Option<String>>>) -> Self {
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

#[derive(Debug, Serialize)]
struct LogEntry {
    pub level: Level,
    pub message: String,
    pub target: String,
    pub module: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl From<log::Level> for Level {
    fn from(value: log::Level) -> Self {
        match value {
            log::Level::Error => Level::Error,
            log::Level::Warn => Level::Warn,
            log::Level::Info => Level::Info,
            log::Level::Debug => Level::Debug,
            log::Level::Trace => Level::Trace,
        }
    }
}

impl From<&log::Record<'_>> for LogEntry {
    fn from(value: &log::Record) -> Self {
        Self {
            level: value.level().into(),
            message: value.args().to_string(),
            target: value.target().to_string(),
            module: value.module_path().unwrap_or_default().to_string(),
            file: value.file().unwrap_or_default().to_string(),
            line: value.line().unwrap_or_default(),
        }
    }
}
