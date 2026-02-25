//! Utilities for handling logging-related functionality in the proto crate,
//! such as converting proto log entries into human-readable strings and
//! converting to corresponding types in the `log` crate when the `log` feature
//! is enabled.

use std::fmt::{Debug, Display};

use crate::v1::{Log as ProtoLog, LogLevel as ProtoLogLevel};

/// Display implementation for the log level of a proto log entry, independent
/// of the `log` crate.
impl Display for ProtoLogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level_str = match self {
            ProtoLogLevel::Unspecified => "UNSPECIFIED",
            ProtoLogLevel::Error => "ERROR",
            ProtoLogLevel::Warn => "WARN",
            ProtoLogLevel::Info => "INFO",
            ProtoLogLevel::Debug => "DEBUG",
            ProtoLogLevel::Trace => "TRACE",
        };

        write!(f, "{}", level_str)
    }
}

/// Convert a proto log entry into a human-readable string in the format
/// `[<LEVEL> <MODULE>] <MESSAGE>`.
impl Display for ProtoLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{} {}] {}", self.level(), self.module, self.message)
    }
}

/// Convert a proto log entry into a human-readable string in the format
/// `[<LEVEL> <MODULE>] <MESSAGE> (<FILE>:<LINE>)`, where the file and line
/// number of the original log entry are included if available.
impl Debug for ProtoLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{} {}] {}", self.level(), self.module, self.message,)?;

        if let Some(location) = &self.location {
            write!(f, " ({}:{})", location.path, location.line)?;
        } else {
            write!(f, " (no location)")?;
        }

        Ok(())
    }
}

#[cfg(any(test, feature = "log"))]
mod log {
    use log::{log, Level};

    use crate::v1::{Log as ProtoLog, LogLevel as ProtoLogLevel};

    impl From<Level> for ProtoLogLevel {
        fn from(level: Level) -> Self {
            match level {
                Level::Error => Self::Error,
                Level::Warn => Self::Warn,
                Level::Info => Self::Info,
                Level::Debug => Self::Debug,
                Level::Trace => Self::Trace,
            }
        }
    }

    impl From<ProtoLogLevel> for Level {
        fn from(level: ProtoLogLevel) -> Self {
            match level {
                ProtoLogLevel::Error => Self::Error,
                ProtoLogLevel::Warn => Self::Warn,
                ProtoLogLevel::Info => Self::Info,
                ProtoLogLevel::Debug => Self::Debug,
                ProtoLogLevel::Trace => Self::Trace,

                // The Unspecified level is used when the log level is not set. In
                // this case, we default to Warn.
                ProtoLogLevel::Unspecified => Self::Warn,
            }
        }
    }

    impl ProtoLog {
        /// Returns the log level from the proto log entry as a `log::Level`.
        pub fn log_level(&self) -> Level {
            self.level().into()
        }

        /// Logs the proto log entry using the `log` crate, with the target set to
        /// `"<target_prefix>::<target>"`.
        ///
        /// Note: the file and line number of the original log entry are not
        /// preserved.
        pub fn log(&self, target_prefix: Option<impl AsRef<str>>) {
            let target = match target_prefix {
                Some(prefix) => format!("{}::{}", prefix.as_ref(), self.target),
                None => self.target.clone(),
            };

            log!(target: &target, self.log_level(), "{}", self.message);
        }
    }
}

#[cfg(any(test, feature = "tracing"))]
mod tracing {
    use tracing::Level;

    use crate::v1::LogLevel as ProtoLogLevel;

    impl From<ProtoLogLevel> for Level {
        fn from(level: ProtoLogLevel) -> Self {
            match level {
                ProtoLogLevel::Error => Self::ERROR,
                ProtoLogLevel::Warn => Self::WARN,
                ProtoLogLevel::Info => Self::INFO,
                ProtoLogLevel::Debug => Self::DEBUG,
                ProtoLogLevel::Trace => Self::TRACE,

                // The Unspecified level is used when the log level is not set. In
                // this case, we default to WARN.
                ProtoLogLevel::Unspecified => Self::WARN,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ::log::Level;
    use ::tracing::Level as TracingLevel;

    use super::*;
    use crate::v1::FileLocation;

    #[test]
    fn display_log_level_all_variants() {
        assert_eq!(ProtoLogLevel::Unspecified.to_string(), "UNSPECIFIED");
        assert_eq!(ProtoLogLevel::Error.to_string(), "ERROR");
        assert_eq!(ProtoLogLevel::Warn.to_string(), "WARN");
        assert_eq!(ProtoLogLevel::Info.to_string(), "INFO");
        assert_eq!(ProtoLogLevel::Debug.to_string(), "DEBUG");
        assert_eq!(ProtoLogLevel::Trace.to_string(), "TRACE");
    }

    #[test]
    fn display_log_entry() {
        let log = ProtoLog {
            level: ProtoLogLevel::Info.into(),
            message: "hello world".into(),
            module: "my_module".into(),
            target: String::new(),
            location: None,
        };

        assert_eq!(format!("{log}"), "[INFO my_module] hello world");
    }

    #[test]
    fn debug_log_entry_without_location() {
        let log = ProtoLog {
            level: ProtoLogLevel::Warn.into(),
            message: "something happened".into(),
            module: "mod_a".into(),
            target: String::new(),
            location: None,
        };

        assert_eq!(
            format!("{log:?}"),
            "[WARN mod_a] something happened (no location)"
        );
    }

    #[test]
    fn debug_log_entry_with_location() {
        let log = ProtoLog {
            level: ProtoLogLevel::Error.into(),
            message: "failure".into(),
            module: "mod_b".into(),
            target: String::new(),
            location: Some(FileLocation {
                path: "src/main.rs".into(),
                line: 42,
            }),
        };

        assert_eq!(format!("{log:?}"), "[ERROR mod_b] failure (src/main.rs:42)");
    }

    #[test]
    fn display_log_with_unspecified_level() {
        let log = ProtoLog {
            level: ProtoLogLevel::Unspecified.into(),
            message: "unknown level".into(),
            module: "mod_c".into(),
            target: String::new(),
            location: None,
        };

        assert_eq!(format!("{log}"), "[UNSPECIFIED mod_c] unknown level");
    }

    #[test]
    fn log_level_to_proto_log_level() {
        assert_eq!(ProtoLogLevel::from(Level::Error), ProtoLogLevel::Error);
        assert_eq!(ProtoLogLevel::from(Level::Warn), ProtoLogLevel::Warn);
        assert_eq!(ProtoLogLevel::from(Level::Info), ProtoLogLevel::Info);
        assert_eq!(ProtoLogLevel::from(Level::Debug), ProtoLogLevel::Debug);
        assert_eq!(ProtoLogLevel::from(Level::Trace), ProtoLogLevel::Trace);
    }

    #[test]
    fn proto_log_level_to_log_level() {
        assert_eq!(Level::from(ProtoLogLevel::Error), Level::Error);
        assert_eq!(Level::from(ProtoLogLevel::Warn), Level::Warn);
        assert_eq!(Level::from(ProtoLogLevel::Info), Level::Info);
        assert_eq!(Level::from(ProtoLogLevel::Debug), Level::Debug);
        assert_eq!(Level::from(ProtoLogLevel::Trace), Level::Trace);
    }

    #[test]
    fn proto_log_level_unspecified_defaults_to_warn() {
        assert_eq!(Level::from(ProtoLogLevel::Unspecified), Level::Warn);
    }

    #[test]
    fn log_level_method() {
        let log = ProtoLog {
            level: ProtoLogLevel::Error.into(),
            message: String::new(),
            module: String::new(),
            target: String::new(),
            location: None,
        };
        assert_eq!(log.log_level(), Level::Error);
    }

    #[test]
    fn log_level_method_unspecified_defaults_to_warn() {
        let log = ProtoLog {
            level: ProtoLogLevel::Unspecified.into(),
            message: String::new(),
            module: String::new(),
            target: String::new(),
            location: None,
        };
        assert_eq!(log.log_level(), Level::Warn);
    }

    #[test]
    fn log_with_target_prefix() {
        let log = ProtoLog {
            level: ProtoLogLevel::Info.into(),
            message: "msg".into(),
            module: String::new(),
            target: "my_target".into(),
            location: None,
        };
        // Should not panic; just exercises the code path.
        log.log(Some("prefix"));
    }

    #[test]
    fn log_without_target_prefix() {
        let log = ProtoLog {
            level: ProtoLogLevel::Info.into(),
            message: "msg".into(),
            module: String::new(),
            target: "my_target".into(),
            location: None,
        };
        log.log(None::<&str>);
    }

    #[test]
    fn test_tracing_level_conversion() {
        assert_eq!(
            TracingLevel::from(ProtoLogLevel::Error),
            TracingLevel::ERROR
        );
        assert_eq!(TracingLevel::from(ProtoLogLevel::Warn), TracingLevel::WARN);
        assert_eq!(TracingLevel::from(ProtoLogLevel::Info), TracingLevel::INFO);
        assert_eq!(
            TracingLevel::from(ProtoLogLevel::Debug),
            TracingLevel::DEBUG
        );
        assert_eq!(
            TracingLevel::from(ProtoLogLevel::Trace),
            TracingLevel::TRACE
        );
        assert_eq!(
            TracingLevel::from(ProtoLogLevel::Unspecified),
            TracingLevel::WARN
        );
    }
}
