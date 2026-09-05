use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    sync::{Arc, RwLock},
    time::Instant,
};

use anyhow::{anyhow, Context, Error};
use chrono::{DateTime, Utc};
use log::{info, trace, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sysinfo::System;
use tracing::{
    field::{Field, Visit},
    span, Event, Subscriber,
};
use tracing_subscriber::{layer::Layer, registry::LookupSpan};

use osutils::{
    files,
    osrelease::{OsRelease, OS_RELEASE_PATH},
    uname,
};

use crate::{logging::operation_context, TRIDENT_METRICS_FILE_PATH, TRIDENT_VERSION};

lazy_static::lazy_static! {
    static ref ADDITIONAL_FIELDS: BTreeMap<String, Value> = populate_additional_fields();
    pub static ref PLATFORM_INFO: BTreeMap<String, Value> = populate_platform_info();
}

#[derive(Default)]
struct TraceEntryVisitor {
    fields: BTreeMap<String, Value>,
}

/// A visitor that records the fields of an event as a BTreeMap This follows the
/// Visitor pattern (see
/// https://docs.rs/tracing-core/latest/tracing_core/field/trait.Visit.html)
/// from the tracing crate to record the fields of an event as a BTreeMap. This
/// is used to create a TraceEntry from the event.
impl Visit for TraceEntryVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name().to_string(), json!(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), json!(format!("{:?}", value)));
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TraceEntry {
    pub timestamp: DateTime<Utc>,
    pub metric_name: String,
    pub value: Value,
    pub additional_fields: BTreeMap<String, Value>,
    pub platform_info: BTreeMap<String, Value>,
}

#[derive(Default, Clone)]
pub struct TraceStream {
    // TODO: Consider changing this to a LockOnce when rustc is updated to
    // >=1.70
    target: Arc<RwLock<Option<String>>>,
    installation_id: Arc<RwLock<Option<String>>>,
    disabled: bool,
}

/// The TraceStream is a struct that holds the target URL for the tracestream
/// and a flag to disable the tracestream. It also has methods to set the server
/// and create a TraceSender.
impl TraceStream {
    /// Permanently disable the tracestream
    ///
    /// Useful for cases when we know we don't want to send traces to the server
    pub fn disable(&mut self) {
        self.disabled = true;
    }

    pub fn set_server(&self, url: String) -> Result<(), Error> {
        if self.disabled {
            info!("tracestream is disabled, ignoring set_server");
            return Ok(());
        }

        reqwest::Url::parse(&url).context(format!("Failed to parse tracestream URL: {url}"))?;
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow!("Failed to lock tracestream"))?;
        val.replace(url);
        Ok(())
    }

    /// Clear the tracestream server URL
    ///
    /// This will stop logs from being sent to the server.
    pub fn clear_server(&self) -> Result<(), Error> {
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow!("Failed to lock tracestream"))?;
        val.take();
        Ok(())
    }

    /// Set the installation ID to attach to every trace entry sent from this point
    /// forward, as an additional field, so that all traces/metrics for a
    /// given host installation can be correlated. Expected to be called once
    /// the datastore's persisted installation ID has been retrieved (see
    /// `DataStore::installation_id`).
    pub fn set_installation_id(&self, installation_id: String) {
        match self.installation_id.write() {
            Ok(mut val) => {
                val.replace(installation_id);
            }
            Err(_) => warn!("Failed to lock tracestream to set installation ID"),
        }
    }

    /// Returns a clone of the shared installation-ID handle -- the same
    /// underlying `Arc<RwLock<..>>` written by `set_installation_id` -- so
    /// other telemetry sinks (namely `AppInsightsSender`) can read the
    /// current value at send-time without needing their own copy of the
    /// logic that sets it.
    pub fn installation_id_handle(&self) -> Arc<RwLock<Option<String>>> {
        self.installation_id.clone()
    }

    /// Create a Boxed TraceSender. Truncates the local metrics file on
    /// creation, same as every previous invocation of a command that
    /// installs this layer -- appropriate for commands that are
    /// themselves generating fresh servicing metrics.
    pub fn make_trace_sender(&self) -> Box<TraceSender> {
        self.make_trace_sender_with_metrics_path(TRIDENT_METRICS_FILE_PATH, true)
    }

    /// Like `make_trace_sender`, but appends to the existing local metrics
    /// file instead of truncating it. For commands (namely `diagnose`)
    /// that read back and repackage that same file's *pre-existing*
    /// content (e.g. into a support bundle) -- truncating it first would
    /// destroy the history the command is supposed to be collecting,
    /// leaving only the metrics the command emits about itself.
    pub fn make_trace_sender_appending(&self) -> Box<TraceSender> {
        self.make_trace_sender_with_metrics_path(TRIDENT_METRICS_FILE_PATH, false)
    }

    /// Like `make_trace_sender`, but writes the local metrics file to
    /// `metrics_file_path` instead of the real host path
    /// (`TRIDENT_METRICS_FILE_PATH`), and lets the caller choose whether
    /// to truncate it first. This lets tests exercise the full
    /// metrics-writing pipeline against a throwaway temp file instead of a
    /// real, shared host path, so they can be plain `#[test]`s instead of
    /// needing a VM.
    pub(crate) fn make_trace_sender_with_metrics_path(
        &self,
        metrics_file_path: &str,
        truncate: bool,
    ) -> Box<TraceSender> {
        Box::new(TraceSender::new(
            self.target.clone(),
            self.installation_id.clone(),
            metrics_file_path,
            truncate,
        ))
    }
}

pub struct TraceSender {
    server: Arc<RwLock<Option<String>>>,
    installation_id: Arc<RwLock<Option<String>>>,
    client: reqwest::blocking::Client,
    metrics_file: Option<File>,
}

struct ExecutionTime(Instant);

/// The TraceSender is a struct that holds the server URL and a reqwest client
/// to send the trace entries to the server. It implements the Layer trait from
/// the tracing-subscriber crate to handle the events and send them to the
/// server.
impl TraceSender {
    fn new(
        server: Arc<RwLock<Option<String>>>,
        installation_id: Arc<RwLock<Option<String>>>,
        metrics_file_path: &str,
        truncate: bool,
    ) -> Self {
        let metrics_file = if truncate {
            files::create_file(metrics_file_path)
        } else {
            if let Some(parent) = Path::new(metrics_file_path).parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    eprintln!(
                        "Tracestream setup error: failed to create local metrics file's parent directory: {err:?}"
                    );
                }
            }
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(metrics_file_path)
                .map_err(Error::from)
        };
        Self {
            server,
            installation_id,
            client: reqwest::blocking::Client::new(),
            metrics_file: match metrics_file {
                Ok(f) => Some(f),
                Err(err) => {
                    eprintln!(
                        "Tracestream setup error: failed to create local metrics file: {err:?}"
                    );
                    None
                }
            },
        }
    }

    fn get_server(&self) -> Option<String> {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// Build the `additional_fields` map for a trace entry: the static
    /// `ADDITIONAL_FIELDS`, the installation ID (if one has been set via
    /// `TraceStream::set_installation_id`), and the current thread's
    /// `operation_id`/`command`/`servicing_id` (if any, see
    /// `operation_context`), so entries can be correlated back to a
    /// specific host installation and servicing operation.
    ///
    /// `operation_id`/`command`/`servicing_id` are deliberately merged here
    /// rather than into the metric's own `value` (as scalar/span fields
    /// are): mixing them into `value` would change the established schema
    /// for simple scalar metrics -- e.g. `clean_install_start` would go
    /// from `"value": true` to `"value": {"command": ..., "operation_id":
    /// ..., "value": true}` the moment it ran inside an operation context,
    /// breaking that contract for existing consumers.
    fn additional_fields(&self) -> BTreeMap<String, Value> {
        let mut fields = ADDITIONAL_FIELDS.clone();
        if let Ok(installation_id) = self.installation_id.read() {
            if let Some(installation_id) = installation_id.as_ref() {
                fields.insert("installation_id".to_string(), json!(installation_id));
            }
        }
        merge_operation_context(&mut fields);
        fields
    }

    fn write_metric_to_file(&self, metric: String) {
        if let Some(mut file) = self.metrics_file.as_ref() {
            if let Err(e) = file.write_all(format!("{metric}\n").as_bytes()) {
                trace!("Failed to write metric to file: {:?}", e);
            }
        }
    }
}

/// The Layer trait from the tracing-subscriber crate is implemented for the
/// TraceSender to handle the events and send them to the server. The enabled
/// function is called for each event to determine if the event should be
/// handled by the TraceSender layer. The on_event function is called for each
/// event to allow the custom layer to process the event and send it to the
/// server.
impl<S> Layer<S> for TraceSender
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    /// Returns true if the event should be handled by the TraceSender layer
    /// Enabled is called for each event
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        metadata.level() <= &tracing::Level::INFO
    }

    /// Each time an event is fired, this function is called for the TraceSender
    /// layer to handle the event and send it to the server. It is called only
    /// if enabled returns true. It creates a TraceEntry from the event based on
    /// the information cared about and sends it to the server.
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = TraceEntryVisitor::default();
        event.record(&mut visitor);

        let metric_name = match visitor.fields.get("metric_name").and_then(|v| v.as_str()) {
            Some(name) => name.to_string(),
            None => {
                warn!("Event does not have a metric_name field, skipping!");
                return;
            }
        };

        // Apart from the metric name, check if we have a single or multiple values
        let filtered_fields: BTreeMap<String, Value> = visitor
            .fields
            .into_iter()
            .filter(|(key, _)| key != "metric_name")
            .collect();
        let value = if filtered_fields.len() > 1 {
            Value::Object(Map::from_iter(filtered_fields))
        } else {
            filtered_fields
                .into_iter()
                .find(|(k, _)| k == "value")
                .map(|(_, v)| v)
                .unwrap_or_default()
        };

        let entry = TraceEntry {
            timestamp: Utc::now(),
            metric_name,
            value: json!(value),
            additional_fields: self.additional_fields(),
            platform_info: PLATFORM_INFO.clone(),
        };

        let body = match serde_json::to_string(&entry) {
            Ok(b) => b,
            Err(e) => {
                trace!("Failed to serialize trace entry: {}", e);
                return;
            }
        };

        // Write the metric to the local metrics file
        self.write_metric_to_file(body.clone());

        // Send the trace entry to the server if it exists
        if let Some(target) = self.get_server() {
            if let Err(e) = self.client.post(target).body(body).send() {
                trace!("Failed to send trace entry: {}", e);
            }
        }
    }

    /// When a new span is created, we want to record any fields that are
    /// attached to it using the visitor pattern.
    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut visitor = TraceEntryVisitor::default();
            attrs.record(&mut visitor);
            span.extensions_mut().insert(visitor);
        }
    }

    /// When a span is entered (either manually or using the tracing macros),
    /// this function is called to handle creating the span with the start time.
    fn on_enter(&self, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            trace!("Failed to get span with id: {:?}", id);
            return;
        };
        span.extensions_mut().insert(ExecutionTime(Instant::now()));
        trace!("Entered span: {:?}", span.name());
    }

    /// When a span is exited, this function is called to handle the span and
    /// set the elapsed time. It will then formulate a metric request and send
    /// the span to the server.
    fn on_exit(&self, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            trace!("Failed to get span with id: {:?}", id);
            return;
        };
        let Some(ExecutionTime(start)) = span.extensions_mut().remove::<ExecutionTime>() else {
            trace!("Failed to get start time for span: {:?}", span.name());
            return;
        };
        let execution_time = start.elapsed().as_secs_f64();
        trace!(
            "Closed span: {:?}, execution_time: {:.2} seconds",
            span.name(),
            execution_time
        );

        let Some(mut visitor) = span.extensions_mut().remove::<TraceEntryVisitor>() else {
            trace!("Failed to get fields for span: {:?}", span.name());
            return;
        };
        visitor
            .fields
            .insert("execution_time".to_string(), json!(execution_time));

        let entry = TraceEntry {
            timestamp: Utc::now(),
            metric_name: span.name().to_string(),
            value: json!(visitor.fields),
            additional_fields: self.additional_fields(),
            platform_info: PLATFORM_INFO.clone(),
        };

        let body = match serde_json::to_string(&entry) {
            Ok(b) => b,
            Err(e) => {
                trace!("Failed to serialize trace entry: {}", e);
                return;
            }
        };

        // Write the metric to the local metrics file
        self.write_metric_to_file(body.clone());

        // Send the trace entry to the server if it exists
        if let Some(target) = self.get_server() {
            if let Err(e) = self.client.post(target).body(body).send() {
                trace!("Failed to send trace entry: {}", e);
            }
        }
    }

    /// When a field wants to be recorded at any time during an active span, this
    /// function is called to handle storing the field with the visitor pattern.
    fn on_record(
        &self,
        id: &span::Id,
        values: &span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            // Get the visitor from the span's extensions that was added during span creation
            if let Some(visitor) = span.extensions_mut().get_mut::<TraceEntryVisitor>() {
                values.record(visitor);
            }
        }
    }
}

/// Merge the current thread's `operation_id`/`command` (see
/// `operation_context`), if any, into `fields`. Values the caller already
/// set (e.g. an event that explicitly names its own `command`) are never
/// overwritten.
fn merge_operation_context(fields: &mut BTreeMap<String, Value>) {
    if let Some((operation_id, command, servicing_id)) = operation_context::current() {
        fields
            .entry("operation_id".to_string())
            .or_insert_with(|| json!(operation_id));
        fields
            .entry("command".to_string())
            .or_insert_with(|| json!(command));
        if let Some(servicing_id) = servicing_id {
            fields
                .entry("servicing_id".to_string())
                .or_insert_with(|| json!(servicing_id));
        }
    }
}

fn populate_additional_fields() -> BTreeMap<String, Value> {
    // TODO: Add more additional fields here as needed
    let mut additional_fields = BTreeMap::new();
    additional_fields.insert("trident_version".to_string(), json!(TRIDENT_VERSION));
    additional_fields
}

/// Grab the os-release file and extract the VERSION field
fn get_os_release() -> String {
    match OsRelease::read().map(|os_rel| os_rel.version) {
        Ok(Some(version)) => return version,
        Ok(None) => {
            warn!(
                "Failed to find 'VERSION' in '{OS_RELEASE_PATH}' file, using 'unknown' as os_release"
            );
        }
        Err(e) => {
            warn!(
                "Failed to read '{OS_RELEASE_PATH}' file, using 'unknown' as os_release: {}",
                e
            );
        }
    }
    "unknown".into()
}

/// Populate the platform info with machine information
fn populate_platform_info() -> BTreeMap<String, Value> {
    let mut platform_info = BTreeMap::new();
    let mut sys = System::new();
    sys.refresh_all();
    platform_info.insert("os_release".to_string(), json!(get_os_release()));
    platform_info.insert("total_cpu".to_string(), json!(sys.cpus().len()));
    platform_info.insert(
        "total_memory_gib".to_string(),
        json!((sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0)).round() as u64),
    );

    let kernel_release = uname::kernel_release().unwrap_or_else(|e| {
        warn!(
            "Failed to get kernel release, using 'unknown' as value: {}",
            e
        );
        "unknown".to_string()
    });
    platform_info.insert("kernel_version".to_string(), json!(kernel_release.trim()));
    platform_info
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::File,
        io::{BufRead, BufReader},
    };

    use tracing_subscriber::{filter, layer::SubscriberExt};

    #[test]
    fn test_tracestream() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender =
            tracestream.make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true);
        assert!(
            trace_sender.get_server().is_none(),
            "tracestream should not have a server"
        );

        tracestream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        assert_eq!(
            trace_sender.get_server().unwrap(),
            "http://localhost:8080",
            "tracestream should have a server"
        );
    }

    #[test]
    /// Regression test: `make_trace_sender_with_metrics_path(.., false)`
    /// (used by `make_trace_sender_appending`, for `diagnose`) must append
    /// to a pre-existing metrics file rather than truncating it -- unlike
    /// the `true` (truncating) case every other command uses.
    fn test_tracestream_appending_sender_preserves_existing_metrics() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        std::fs::write(&metrics_path, "preexisting line\n").unwrap();

        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), false)
            .with_filter(filter::LevelFilter::INFO);

        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        );

        tracing::info!(metric_name = "test_metric_appended", value = true);

        std::thread::sleep(std::time::Duration::from_millis(100));

        let file = File::open(&metrics_path).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        assert!(
            lines.iter().any(|line| line == "preexisting line"),
            "appending sender must not have truncated the pre-existing content"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains(r#""metric_name":"test_metric_appended""#)),
            "appending sender must still write new metrics"
        );
    }

    #[test]
    fn test_lock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let mut tracestream = TraceStream::default();
        let trace_sender =
            tracestream.make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true);

        assert!(
            trace_sender.get_server().is_none(),
            "tracestream should not have a server"
        );

        tracestream.disable();

        tracestream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        assert!(
            trace_sender.get_server().is_none(),
            "tracestream should not have a server"
        );
    }

    #[test]
    fn test_tracestream_write_metric_event_to_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true)
            .with_filter(filter::LevelFilter::INFO);

        // Use a thread-local scoped default subscriber (rather than
        // `set_global_default`) since this is a plain `#[test]` that may run
        // concurrently with other tests in the same process -- the global
        // default can only be set once per process, but the scoped default
        // is per-thread and automatically restored when `_guard` drops.
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        );

        tracing::info!(metric_name = "test_metric", value = true);

        // Ensure the trace system has time to write the file.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check if the specific metric exists in the file.
        let file = File::open(&metrics_path).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        let expected_substring = r#""metric_name":"test_metric","value":true"#;
        let metric_found = lines.iter().any(|line| line.contains(expected_substring));

        // Assert that the expected metric is present in the file.
        assert!(
            metric_found,
            "Expected test metric not found in the local metrics file"
        );
    }

    #[test]
    /// Regression test: `TraceStream::set_installation_id` must actually
    /// reach the serialized trace entry's `additional_fields.installation_id`
    /// -- the metric/span tests above only assert on `metric_name`/`value`
    /// and would still pass even if the installation ID were never copied
    /// into `additional_fields`.
    fn test_tracestream_installation_id_written_to_additional_fields() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        tracestream.set_installation_id("test-installation-id".to_string());
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true)
            .with_filter(filter::LevelFilter::INFO);

        // See test_tracestream_write_metric_event_to_file for why a scoped
        // (not global) default subscriber is used here.
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        );

        tracing::info!(
            metric_name = "test_metric_with_installation_id",
            value = true
        );

        // Ensure the trace system has time to write the file.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let file = File::open(&metrics_path).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        let metric_found = lines.iter().any(|line| {
            line.contains(r#""metric_name":"test_metric_with_installation_id""#)
                && line.contains(r#""installation_id":"test-installation-id""#)
        });

        assert!(
            metric_found,
            "Expected metric with installation_id field not found in the local metrics file"
        );
    }

    #[test]
    /// Regression test: `operation_context::set_servicing_id` must actually
    /// reach the serialized trace entry's `additional_fields.servicing_id`
    /// -- mirrors `test_tracestream_installation_id_written_to_additional_fields`
    /// above, but for the persistent per-servicing-operation ID instead of
    /// the per-install one.
    fn test_tracestream_servicing_id_written_to_additional_fields() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true)
            .with_filter(filter::LevelFilter::INFO);

        // See test_tracestream_write_metric_event_to_file for why a scoped
        // (not global) default subscriber is used here.
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        );

        operation_context::run_with_operation("test_command", || {
            operation_context::set_servicing_id("test-servicing-id");
            tracing::info!(metric_name = "test_metric_with_servicing_id", value = true);
        });

        // Ensure the trace system has time to write the file.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let file = File::open(&metrics_path).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        let metric_found = lines.iter().any(|line| {
            line.contains(r#""metric_name":"test_metric_with_servicing_id""#)
                && line.contains(r#""servicing_id":"test-servicing-id""#)
        });

        assert!(
            metric_found,
            "Expected metric with servicing_id field not found in the local metrics file"
        );
    }

    #[test]
    fn test_tracestream_write_span_metric_to_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true)
            .with_filter(filter::LevelFilter::INFO);

        // See test_tracestream_write_metric_event_to_file for why a scoped
        // (not global) default subscriber is used here.
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        );

        // Call test function that will create a span
        simulate_function_span();

        // Ensure the trace system has time to simulate a span.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check if the specific metric exists in the file.
        let file = File::open(&metrics_path).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        let expected_substring = r#""metric_name":"test_span"#;
        let span_metric_found = lines.iter().any(|line| line.contains(expected_substring));

        // Assert that the expected metric is present in the file.
        assert!(
            span_metric_found,
            "Expected test metric not found in the local metrics file"
        );
    }

    // Helper function to test span metrics
    #[tracing::instrument(name = "test_span", skip_all)]
    fn simulate_function_span() {}
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    // These two remain functional tests (VM-only) because they assert
    // against the actual host's hardware/platform info (CPU count, memory,
    // product UUID, os-release, kernel version) -- unlike the metrics-file
    // tests above, there's no way to inject a fake value here, so the
    // result is inherently host-dependent.

    #[functional_test]
    fn test_populate_additional_fields() {
        let additional_fields = populate_additional_fields();
        assert_eq!(
            additional_fields.get("trident_version").unwrap(),
            &json!(TRIDENT_VERSION)
        );
    }

    #[functional_test]
    fn test_populate_platform_info() {
        let mut expected_platform_info = BTreeMap::new();
        expected_platform_info.insert("os_release".to_string(), json!(get_os_release()));
        expected_platform_info.insert("total_cpu".to_string(), json!(4));
        expected_platform_info.insert("total_memory_gib".to_string(), json!(6));
        expected_platform_info.insert(
            "kernel_version".to_string(),
            json!(uname::kernel_release().unwrap().trim()),
        );

        // Call the function to get the actual result.
        let platform_info = populate_platform_info();

        // Assert that the actual result matches the expected result.
        assert_eq!(
            platform_info, expected_platform_info,
            "Platform info does not match the expected result"
        );
    }
}
