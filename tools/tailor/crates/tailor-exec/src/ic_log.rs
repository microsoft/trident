//! Structured Image Customizer log handling (`meta/docs/2026-06-29-logging.md` §5).
//!
//! IC runs with `--log-format=json --log-level=debug --log-color=never`, so each stderr line is one
//! logrus object `{"level":..,"msg":..,"time":..}`. This module turns that stream into something
//! tailor controls: it parses each line, re-emits it as a `tracing` event at the mapped level under
//! target `ic` for real IC output (or `janitor` for tailor's own cleanup — see [`emit`]), so tailor's
//! existing verbosity filter governs the live display and IC-attributed lines are only ever genuine
//! IC output; keeps every line in
//! an always-on in-memory capture, and — keyed off the **non-zero exit code** — renders a
//! categorized, bounded failure dump from that capture.

use std::{fmt::Write as _, path::Path};

use tailor_core::LogSource;
use tracing::{debug, error, info, trace, warn};

/// How many `info`/`warn` context lines to include in a failure dump (§5.4).
const CONTEXT_TAIL: usize = 50;

/// The tracing target real Image Customizer output carries, so `RUST_LOG=ic=debug` works and IC
/// output can be filtered separately from tailor's own logs (§5.3).
const TARGET_IC: &str = "ic";

/// The tracing target for tailor's ownership/cleanup janitor (`chown`/`rm`). Keeping it distinct from
/// `ic` ensures IC-attributed lines are only ever genuine IC output — a janitor `rm` warning must
/// never appear as `ic:` (§5.3).
const TARGET_JANITOR: &str = "janitor";

/// A logrus level mapped to the `tracing` level tailor re-emits it at (§5.3). `fatal`/`panic`
/// collapse into `Error` (tracing has no `fatal`); the original label is retained for the dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IcLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl IcLevel {
    /// Map a logrus `level` string onto a tracing level. Unknown/`fatal`/`panic` → `Error` so a
    /// failure is never silently downgraded.
    fn from_logrus(level: &str) -> Self {
        match level {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warn" | "warning" => Self::Warn,
            // "error" | "fatal" | "panic" | anything unexpected
            _ => Self::Error,
        }
    }
}

/// One captured IC output line: a parsed logrus object, or a non-JSON line kept verbatim (the engine's
/// platform warning, IC's arg-parse `imagecustomizer: error: …`, or any pre-logrus startup text) (§5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IcLine {
    /// A parsed logrus JSON line. `logrus_level` is the original label (`info`, `fatal`, …).
    Structured {
        level: IcLevel,
        logrus_level: String,
        msg: String,
    },
    /// A non-JSON line, kept exactly as IC/the engine emitted it.
    Raw(String),
}

/// Parse one IC stderr line. A `{...}` object carrying both `level` and `msg` becomes `Structured`;
/// anything else (including malformed JSON) is preserved verbatim as `Raw` — nothing is dropped.
pub(crate) fn parse_ic_line(raw: &str) -> IcLine {
    let trimmed = raw.trim_end_matches(['\n', '\r']);
    parse_structured(trimmed).unwrap_or_else(|| IcLine::Raw(trimmed.to_owned()))
}

/// Try to read one logrus object: a JSON object with both a string `level` and a string `msg`.
fn parse_structured(line: &str) -> Option<IcLine> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let level = value.get("level")?.as_str()?;
    let msg = value.get("msg")?.as_str()?;
    Some(IcLine::Structured {
        level: IcLevel::from_logrus(level),
        logrus_level: level.to_owned(),
        msg: msg.to_owned(),
    })
}

/// Re-emit one parsed line at a fixed tracing `target` (which must be a literal, as it is baked into
/// the callsite metadata). Shared by [`emit`] across sources so only the target differs.
macro_rules! emit_at {
    ($target:expr, $line:expr, $cell:expr) => {{
        let cell = $cell;
        match $line {
            IcLine::Structured { level, msg, .. } => match level {
                IcLevel::Trace => trace!(target: $target, cell, "{msg}"),
                IcLevel::Debug => debug!(target: $target, cell, "{msg}"),
                IcLevel::Info => info!(target: $target, cell, "{msg}"),
                IcLevel::Warn => warn!(target: $target, cell, "{msg}"),
                IcLevel::Error => error!(target: $target, cell, "{msg}"),
            },
            IcLine::Raw(text) => warn!(target: $target, cell, "{text}"),
        }
    }};
}

/// Re-emit one parsed line as a `tracing` event at its mapped level, tagged with the cell slug, under
/// the target for `source` (`ic` for real Image Customizer output, `janitor` for tailor's cleanup).
/// Non-JSON lines surface at `WARN` (their failure role is handled in the dump, §5.4).
pub(crate) fn emit(line: &IcLine, cell: &str, source: LogSource) {
    match source {
        LogSource::ImageCustomizer => emit_at!(TARGET_IC, line, cell),
        LogSource::Janitor => emit_at!(TARGET_JANITOR, line, cell),
    }
}

/// The always-on in-memory capture of one cell's IC output, in arrival order (§5.4). Powers the
/// categorized failure dump; the live display is handled separately by [`emit`].
#[derive(Debug, Default)]
pub(crate) struct IcCapture {
    lines: Vec<IcLine>,
}

impl IcCapture {
    pub(crate) fn push(&mut self, line: IcLine) {
        self.lines.push(line);
    }

    /// All captured lines joined verbatim, for non-IC (e.g. janitor) error reporting and debugging.
    pub(crate) fn joined(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                IcLine::Structured { msg, .. } => out.push_str(msg),
                IcLine::Raw(text) => out.push_str(text),
            }
            out.push('\n');
        }
        out
    }

    /// Render the categorized, bounded failure report for a non-zero exit (§5.4):
    ///
    /// 1. **Cause**, keyed off the exit code — the last logrus `error`/`fatal`/`panic` `msg` (runtime
    ///    path, multi-line preserved) **else** the trailing non-JSON stderr (arg-parse path).
    /// 2. A **tail** (last [`CONTEXT_TAIL`]) of preceding `info`/`warn` context — only on the
    ///    structured path, where such context exists.
    /// 3. A re-run hint, plus an on-disk **pointer** when persistence is enabled (`log_file`).
    ///
    /// Every line is indented two spaces so the whole block nests cleanly under the `error:` header.
    pub(crate) fn failure_dump(&self, _code: i64, log_file: Option<&Path>) -> String {
        let mut out = String::new();
        match self.cause() {
            Some(Cause::Structured(msg)) => {
                push_indented(&mut out, msg);
                self.push_context_tail(&mut out);
                push_hint(&mut out, log_file);
            }
            Some(Cause::Raw(text)) => {
                push_indented(&mut out, text);
                out.push('\n');
                out.push_str(
                    "  (no structured IC log was produced — cause taken from IC's stderr; usually a \
                     tailor/IC argument\n   mismatch.)\n",
                );
                if let Some(path) = log_file {
                    let _ = writeln!(out, "  full log: {}", path.display());
                }
            }
            None => {
                out.push_str("  (no IC output was captured.)\n");
                if let Some(path) = log_file {
                    let _ = writeln!(out, "  full log: {}", path.display());
                }
            }
        }
        out
    }

    /// Determine the failure cause: a captured logrus `error`/`fatal`/`panic` `msg` wins (runtime
    /// path); otherwise the trailing non-JSON stderr line (arg-parse path).
    fn cause(&self) -> Option<Cause<'_>> {
        if let Some(msg) = self.lines.iter().rev().find_map(|line| match line {
            IcLine::Structured {
                level: IcLevel::Error,
                msg,
                ..
            } => Some(msg.as_str()),
            _ => None,
        }) {
            return Some(Cause::Structured(msg));
        }
        self.lines
            .iter()
            .rev()
            .find_map(|line| match line {
                IcLine::Raw(text) if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .map(Cause::Raw)
    }

    /// Append the bounded `info`/`warn` context block (last [`CONTEXT_TAIL`] lines), if any.
    fn push_context_tail(&self, out: &mut String) {
        let mut context: Vec<String> = self
            .lines
            .iter()
            .filter_map(|line| match line {
                IcLine::Structured {
                    level: IcLevel::Info | IcLevel::Warn,
                    logrus_level,
                    msg,
                } => {
                    // Keep the tail compact: one row per line, the first line of any multi-line msg.
                    let head = msg.lines().next().unwrap_or("");
                    Some(format!("    {:<5} {head}", logrus_level.to_uppercase()))
                }
                _ => None,
            })
            .collect();
        if context.is_empty() {
            return;
        }
        if context.len() > CONTEXT_TAIL {
            context.drain(..context.len() - CONTEXT_TAIL);
        }
        out.push_str("\n  last IC context:\n");
        for row in context {
            out.push_str(&row);
            out.push('\n');
        }
    }
}

/// Append the closing re-run hint and the on-disk pointer when persistence is enabled.
fn push_hint(out: &mut String, log_file: Option<&Path>) {
    match log_file {
        Some(path) => {
            out.push_str("\n  re-run with -vv for full IC debug.\n");
            let _ = writeln!(out, "  full log: {}", path.display());
        }
        None => {
            out.push_str(
                "\n  re-run with -vv for full IC debug, or set TAILOR_LOG_DIR=<dir> to persist \
                 the per-cell log.\n",
            );
        }
    }
}

/// The extracted failure cause (borrowed from the capture).
enum Cause<'a> {
    /// A logrus `error`/`fatal`/`panic` `msg` (possibly multi-line).
    Structured(&'a str),
    /// A trailing non-JSON stderr line.
    Raw(&'a str),
}

/// Push `text` into `out`, indenting every (possibly embedded-`\n`) line two spaces.
fn push_indented(out: &mut String, text: &str) {
    for line in text.split('\n') {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use tracing::subscriber::with_default;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::{Layer, Registry};

    /// A minimal tracing layer that records the `target` of every event, so a test can assert which
    /// target (`ic` vs `janitor`) a re-emitted line was attributed to.
    #[derive(Clone, Default)]
    struct TargetCapture(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for TargetCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(event.metadata().target().to_owned());
        }
    }

    fn targets_of(source: LogSource) -> Vec<String> {
        let capture = TargetCapture::default();
        let subscriber = Registry::default().with(capture.clone());
        with_default(subscriber, || {
            emit(
                &IcLine::Raw("/bin/rm: cannot remove: busy".to_owned()),
                "",
                source,
            );
            emit(
                &IcLine::Structured {
                    level: IcLevel::Info,
                    logrus_level: "info".to_owned(),
                    msg: "hello".to_owned(),
                },
                "gizmo_amd64_cosi",
                source,
            );
        });
        capture.0.lock().unwrap().clone()
    }

    #[test]
    fn janitor_output_is_attributed_to_janitor_not_ic() {
        let targets = targets_of(LogSource::Janitor);
        assert!(!targets.is_empty());
        assert!(
            targets.iter().all(|target| target == "janitor"),
            "janitor lines must never be attributed to `ic`, got {targets:?}"
        );
    }

    #[test]
    fn image_customizer_output_is_attributed_to_ic() {
        let targets = targets_of(LogSource::ImageCustomizer);
        assert!(!targets.is_empty());
        assert!(
            targets.iter().all(|target| target == "ic"),
            "IC lines must be attributed to `ic`, got {targets:?}"
        );
    }

    #[test]
    fn parses_logrus_json_into_mapped_levels() {
        let cases = [
            ("trace", IcLevel::Trace),
            ("debug", IcLevel::Debug),
            ("info", IcLevel::Info),
            ("warn", IcLevel::Warn),
            ("warning", IcLevel::Warn),
            ("error", IcLevel::Error),
            ("fatal", IcLevel::Error),
            ("panic", IcLevel::Error),
        ];
        for (logrus, expected) in cases {
            let raw =
                format!(r#"{{"level":"{logrus}","msg":"hello","time":"2026-06-24T19:54:00Z"}}"#);
            match parse_ic_line(&raw) {
                IcLine::Structured {
                    level,
                    msg,
                    logrus_level,
                } => {
                    assert_eq!(level, expected, "level mapping for {logrus}");
                    assert_eq!(msg, "hello");
                    assert_eq!(logrus_level, logrus);
                }
                IcLine::Raw(other) => panic!("expected structured, got raw: {other}"),
            }
        }
    }

    #[test]
    fn preserves_multiline_msg_verbatim() {
        let raw = r#"{"level":"fatal","msg":"image customization failed:\nopen /nope.yaml: no such file","time":"t"}"#;
        match parse_ic_line(raw) {
            IcLine::Structured { msg, level, .. } => {
                assert_eq!(level, IcLevel::Error);
                assert_eq!(
                    msg,
                    "image customization failed:\nopen /nope.yaml: no such file"
                );
            }
            IcLine::Raw(other) => panic!("expected structured, got raw: {other}"),
        }
    }

    #[test]
    fn non_json_lines_pass_through_as_raw() {
        for raw in [
            "imagecustomizer: error: missing flags: --build-dir=STRING",
            "WARNING: The requested image's platform (linux/amd64) does not match …",
            "{ this is not valid json",
            "{\"level\":\"info\"}", // missing msg → not structured
        ] {
            assert_eq!(parse_ic_line(raw), IcLine::Raw(raw.trim_end().to_owned()));
        }
    }

    fn structured(level: &str, msg: &str) -> IcLine {
        match parse_ic_line(&format!(r#"{{"level":"{level}","msg":"{msg}"}}"#)) {
            line @ IcLine::Structured { .. } => line,
            IcLine::Raw(other) => panic!("expected structured, got raw: {other}"),
        }
    }

    #[test]
    fn failure_dump_uses_fatal_msg_keyed_off_exit_code() {
        let mut capture = IcCapture::default();
        capture.push(structured("info", "Refreshing package metadata"));
        capture.push(structured(
            "info",
            "Installing packages (1): [openssh-server]",
        ));
        capture.push(structured("warn", "Low free disk space 4% (20 MiB) on (/)"));
        capture.push(IcLine::Structured {
            level: IcLevel::Error,
            logrus_level: "fatal".to_owned(),
            msg: "image customization failed:\nout of disk space".to_owned(),
        });

        let dump = capture.failure_dump(1, None);

        // Cause from the fatal msg, multi-line rendered.
        assert!(dump.contains("  image customization failed:"));
        assert!(dump.contains("  out of disk space"));
        // Bounded info/warn context tail.
        assert!(dump.contains("last IC context:"));
        assert!(dump.contains("INFO  Refreshing package metadata"));
        assert!(dump.contains("WARN  Low free disk space"));
        // No persistence → the TAILOR_LOG_DIR hint, no pointer.
        assert!(dump.contains("set TAILOR_LOG_DIR"));
        assert!(!dump.contains("full log:"));
    }

    #[test]
    fn failure_dump_falls_back_to_non_json_stderr() {
        // No logrus error line: an arg-parse failure produces only non-JSON stderr (§4.1).
        let mut capture = IcCapture::default();
        capture.push(IcLine::Raw(
            "WARNING: The requested image's platform does not match".to_owned(),
        ));
        capture.push(IcLine::Raw(
            "imagecustomizer: error: missing flags: --build-dir=STRING".to_owned(),
        ));

        let dump = capture.failure_dump(80, None);

        // Cause taken from the trailing non-JSON stderr line.
        assert!(dump.contains("imagecustomizer: error: missing flags: --build-dir=STRING"));
        assert!(dump.contains("no structured IC log was produced"));
        // No structured context tail on the fallback path.
        assert!(!dump.contains("last IC context"));
    }

    #[test]
    fn failure_dump_points_at_the_on_disk_log_when_persisting() {
        let mut capture = IcCapture::default();
        capture.push(structured("info", "Refreshing package metadata"));
        capture.push(IcLine::Structured {
            level: IcLevel::Error,
            logrus_level: "fatal".to_owned(),
            msg: "boom".to_owned(),
        });

        let path = Path::new("/logs/appliance_amd64_cosi.log");
        let dump = capture.failure_dump(1, Some(path));

        assert!(dump.contains("full log: /logs/appliance_amd64_cosi.log"));
        // With a pointer present, the TAILOR_LOG_DIR suggestion is suppressed.
        assert!(!dump.contains("set TAILOR_LOG_DIR"));
    }
}
