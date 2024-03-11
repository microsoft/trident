use serde::{Deserialize, Serialize};

pub(super) mod background_log;
pub(super) mod logstream;
pub(super) mod multilog;

#[derive(Debug, Serialize, Deserialize)]
struct LogEntry {
    pub level: Level,
    pub message: String,
    pub target: String,
    pub module: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_entry() {
        let entry = LogEntry::from(
            &log::Record::builder()
                .args(format_args!("test_message"))
                .level(log::Level::Info)
                .target("test_target")
                .module_path(Some("test_module"))
                .file(Some("test_file.rs"))
                .line(Some(1))
                .build(),
        );

        assert_eq!(entry.level, Level::Info);
        assert_eq!(entry.message, "test_message");
        assert_eq!(entry.target, "test_target");
        assert_eq!(entry.module, "test_module");
        assert_eq!(entry.file, "test_file.rs");
        assert_eq!(entry.line, 1);
    }
}
