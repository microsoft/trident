//! Complete diagnostic records and output, independent of the UI and servicing policy.

use std::{borrow::Cow, time::Duration};

use log::{Level, LevelFilter};

use trident_client::{LogLevel, ServicingEvent};

use crate::{client::Event, source};

mod output;

pub(crate) use output::Output;

/// Elapsed prefixes show total minutes rather than wrapping after an hour.
const SECONDS_PER_MINUTE: u64 = 60;

/// Distinguishes local diagnostics from records received from Trident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Inst,
    Trident,
}

impl Source {
    /// Stable source tag used by the TUI, plain output and serial mirrors.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Inst => "INST",
            Self::Trident => "TRIDENT",
        }
    }
}

/// Known installation result, used for presentation without storing terminal colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultKind {
    Success,
    Failure,
}

/// One complete sanitized diagnostic; frontends may retain a shorter display copy.
#[derive(Debug, Clone)]
pub(crate) struct Record {
    /// Time since this attempt began.
    pub(crate) elapsed: Duration,
    /// Identifies the producer in every output mode.
    pub(crate) source: Source,
    /// Consumers filter independently; filtering never changes the original record.
    pub(crate) level: LogLevel,
    /// Complete message, including original line breaks and indentation.
    pub(crate) message: String,
    /// Confirmed result records remain visible even with restrictive display filtering.
    pub(crate) result: Option<ResultKind>,
}

impl Record {
    /// Formats total minutes and seconds plus the opening source bracket, for styled prefixes.
    pub(crate) fn prefix(&self) -> String {
        let seconds = self.elapsed.as_secs();
        format!(
            "{:02}:{:02} [",
            seconds / SECONDS_PER_MINUTE,
            seconds % SECONDS_PER_MINUTE
        )
    }

    /// Sanitizes control characters and prefixes every line for plain and serial output.
    pub(crate) fn plain(&self) -> String {
        let prefix = format!(
            "{}{}:{}] ",
            self.prefix(),
            self.source.label(),
            level_label(self.level)
        );
        display_text(&self.message)
            .split('\n')
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Removes terminal control characters while retaining newlines and expanding tabs.
pub(crate) fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}

/// Applies the display filter, treating unknown daemon severities as warnings.
pub(crate) fn log_visible(level: LogLevel, verbosity: LevelFilter) -> bool {
    let level = match level {
        LogLevel::Error => Level::Error,
        LogLevel::Warn | LogLevel::Unspecified | LogLevel::Unknown(_) => Level::Warn,
        LogLevel::Info => Level::Info,
        LogLevel::Debug => Level::Debug,
        LogLevel::Trace => Level::Trace,
        _ => Level::Warn,
    };
    level.to_level_filter() <= verbosity
}

/// Preserves the uppercase severity names used by TUI and serial records.
pub(crate) fn level_label(level: LogLevel) -> Cow<'static, str> {
    match level {
        LogLevel::Error => Cow::Borrowed("ERROR"),
        LogLevel::Warn => Cow::Borrowed("WARN"),
        LogLevel::Info => Cow::Borrowed("INFO"),
        LogLevel::Debug => Cow::Borrowed("DEBUG"),
        LogLevel::Trace => Cow::Borrowed("TRACE"),
        LogLevel::Unspecified => Cow::Borrowed("UNSPECIFIED"),
        LogLevel::Unknown(value) => Cow::Owned(format!("UNKNOWN({value})")),
        _ => Cow::Borrowed("UNSUPPORTED"),
    }
}

/// Sanitizes received diagnostics before persistence, display, or serial mirroring.
pub(crate) fn redact_event(mut event: Event) -> Event {
    match &mut event {
        Event::InstallerLog { message, .. } | Event::Preparing(message) => {
            *message = source::redact_message_urls(message);
        }
        Event::Prepared { description, .. } => {
            *description = source::redact_message_urls(description);
        }
        Event::Error { details, .. } => {
            *details = source::redact_message_urls(details);
        }
        Event::Response(ServicingEvent::Log(record)) => {
            record.message = source::redact_message_urls(&record.message);
        }
        Event::Response(ServicingEvent::Completed(Err(error))) => {
            error.message = source::redact_message_urls(&error.message);
            error.error_message = source::redact_message_urls(&error.error_message);
        }
        _ => {}
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::{self, File};

    use tempfile::TempDir;

    use trident_client::{LogRecord, RemoteError, RemoteErrorCategory};

    use crate::{engine::State, serial::SerialMirrors};

    /// Trident's COSI-loading messages must not expose signed URLs on any installer output.
    #[test]
    fn daemon_image_urls_are_redacted_before_recording_or_mirroring() {
        let root = TempDir::new().unwrap();
        let diagnostic = root.path().join("installer.log");
        let serial = root.path().join("serial.log");
        let mut mirrors = SerialMirrors::default();
        mirrors.push(&serial, File::create(&serial).unwrap());
        let mut output = Output {
            log: Some(File::create(&diagnostic).unwrap()),
            mirrors,
        };
        let mut model = State::new();
        for message in [
            "Attempting to load COSI file from 'https://user:password@example.test/image.cosi?sig=secret#fragment'",
            "Loaded COSI file from 'https://user:password@example.test/image.cosi?sig=secret#fragment' with hash 'abc'",
        ] {
            let event = redact_event(Event::Response(ServicingEvent::Log(LogRecord {
                timestamp: None,
                level: LogLevel::Info,
                message: message.into(),
                module: String::new(),
                target: String::new(),
                location: None,
            })));
            output.record(&event).unwrap();
            let entry = model.event(event);
            output.announce(&entry, model.serial_verbosity);
            assert!(entry.message.contains("https://example.test/image.cosi"));
            for secret in ["user", "password", "sig=secret", "fragment"] {
                assert!(!entry.message.contains(secret), "{entry:?}");
            }
        }
        for text in [
            fs::read_to_string(diagnostic).unwrap(),
            fs::read_to_string(serial).unwrap(),
        ] {
            assert!(text.contains("https://example.test/image.cosi"), "{text}");
            for secret in ["user", "password", "sig=secret", "fragment"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    /// Daemon failures cannot reintroduce a signed URL through structured error details.
    #[test]
    fn daemon_image_errors_redact_signed_urls() {
        let event = redact_event(Event::Response(ServicingEvent::Completed(Err(
            RemoteError {
                category: RemoteErrorCategory::Servicing,
                message: "Failed to load 'https://user:password@example.test/image.cosi?sig=secret#fragment'".into(),
                error_message: "HTTP failure at https://user:password@example.test/image.cosi?sig=secret#fragment".into(),
                subkind: "image-load-failed".into(),
                location: None,
            },
        ))));
        let mut model = State::new();
        let entry = model.event(event);
        assert!(model.details.contains("https://example.test/image.cosi"));
        for secret in ["user", "password", "sig=secret", "fragment"] {
            assert!(!model.details.contains(secret), "{}", model.details);
            assert!(!entry.plain().contains(secret));
        }
    }
}
