use crate::telemetry::fields;
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    path::Path,
    sync::{Arc, RwLock},
    time::Instant,
};

use anyhow::{anyhow, Context, Error};
use chrono::{DateTime, Utc};
use log::{debug, info, trace, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sysinfo::System;
use tracing::{
    field::{Field, Visit},
    span, Event, Subscriber,
};
use tracing_subscriber::{layer::Layer, registry::LookupSpan};

#[cfg(test)]
use crate::command_kind::CommandKind;
use trident_api::error::TridentError;

use osutils::{
    dmi, files,
    osrelease::{OsRelease, OS_RELEASE_PATH},
    uname,
};

use crate::{
    datastore::DataStore, logging::operation_context, TRIDENT_METRICS_FILE_PATH, TRIDENT_VERSION,
};

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

    /// Set the installation ID to attach to every trace entry sent from this
    /// point forward, as an additional field, so that all traces/metrics for
    /// a given host installation can be correlated. Expected to be called
    /// once the datastore's persisted installation ID has been retrieved
    /// (see [`Self::attach_ids_if_present`] and
    /// [`Self::ensure_and_attach_installation_id`]).
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

    fn installation_id_cached(&self) -> bool {
        self.installation_id
            .read()
            .map(|v| v.is_some())
            .unwrap_or(false)
    }

    /// Attach an existing installation ID, migrating provisioned legacy
    /// datastores when needed. Never creates a datastore. Multiboot
    /// callers may defer attachment until their datastore is selected.
    pub fn attach_ids_if_present(&self, datastore_path: &Path, skip_installation_id: bool) {
        if !datastore_path.exists() {
            return;
        }
        let mut datastore = match DataStore::open(datastore_path) {
            Ok(ds) => ds,
            Err(e) => {
                warn!("Failed to open datastore to attach IDs: {e:?}");
                return;
            }
        };

        if !skip_installation_id && !self.installation_id_cached() {
            match datastore.installation_id_or_migrate() {
                Ok(Some(installation_id)) => {
                    info!("Installation ID: {installation_id}");
                    self.set_installation_id(installation_id.to_string());
                }
                Ok(None) => {
                    debug!("No installation ID persisted yet (host not yet installed)");
                }
                Err(e) => {
                    warn!("Failed to read installation ID: {e:?}");
                }
            }
        }
    }

    /// Ensures `datastore` has an installation ID -- creating one if this
    /// is the first access, or reading back the existing one otherwise --
    /// and attaches it. Unlike
    /// [`Self::attach_ids_if_present`], this is only for the
    /// one caller that already knows a command genuinely allowed to
    /// initialize a brand-new datastore (per
    /// [`crate::datastore::DataStore::may_initialize_datastore_for_command`])
    /// is proceeding, and already holds (or just created) the datastore
    /// handle for it -- so this attaches the new install/update's own ID
    /// instead of leaving the trace stream untagged until some later
    /// read-only attach happens to run.
    pub fn ensure_and_attach_installation_id(
        &self,
        datastore: &mut DataStore,
    ) -> Result<(), TridentError> {
        if self.installation_id_cached() {
            return Ok(());
        }
        let installation_id = datastore.ensure_installation_id()?;
        info!("Installation ID: {installation_id}");
        self.set_installation_id(installation_id.to_string());
        Ok(())
    }

    /// Create a Boxed TraceSender
    pub fn make_trace_sender(&self) -> Box<TraceSender> {
        self.make_trace_sender_with_metrics_path(TRIDENT_METRICS_FILE_PATH)
    }

    /// Like `make_trace_sender`, but writes the local metrics file to
    /// `metrics_file_path` instead of the real host path
    /// (`TRIDENT_METRICS_FILE_PATH`). This lets tests exercise the full
    /// metrics-writing pipeline against a throwaway temp file instead of a
    /// real, shared host path, so they can be plain `#[test]`s instead of
    /// needing a VM.
    fn make_trace_sender_with_metrics_path(&self, metrics_file_path: &str) -> Box<TraceSender> {
        Box::new(TraceSender::new(
            self.target.clone(),
            self.installation_id.clone(),
            metrics_file_path,
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
    ) -> Self {
        Self {
            server,
            installation_id,
            client: reqwest::blocking::Client::new(),
            metrics_file: match files::create_file(metrics_file_path) {
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

    fn additional_fields(&self) -> BTreeMap<String, Value> {
        let mut fields = ADDITIONAL_FIELDS.clone();
        if let Ok(installation_id) = self.installation_id.read() {
            if let Some(installation_id) = installation_id.as_ref() {
                fields.insert(fields::INSTALLATION_ID.to_string(), json!(installation_id));
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

        let metric_name = match visitor
            .fields
            .get(fields::METRIC_NAME)
            .and_then(|v| v.as_str())
        {
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
            .filter(|(key, _)| key != fields::METRIC_NAME)
            .collect();
        let value = if filtered_fields.len() > 1 {
            Value::Object(Map::from_iter(filtered_fields))
        } else {
            filtered_fields
                .into_iter()
                .find(|(k, _)| k == fields::VALUE)
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
            .insert(fields::EXECUTION_TIME.to_string(), json!(execution_time));

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

/// Obtain product uuid of the hardware Trident is running on. Falls back to
/// "unknown" (rather than failing) since this value is purely informational
/// metadata attached to trace entries.
///
/// Takes the raw string from [`dmi::read_product_uuid_raw`], not the typed
/// [`OsUuid`] from [`dmi::read_product_uuid`]: `OsUuid`'s `Display`
/// canonicalizes a well-formed UUID to lowercase, which would silently
/// change this `asset_id`'s representation (and break continuity with prior
/// telemetry for the same asset) on any host whose BIOS reports
/// `product_uuid` in uppercase.
fn product_uuid_or_unknown(uuid: Result<String, Error>) -> String {
    uuid.unwrap_or_else(|err| {
        debug!("Failed to read product uuid: {err:#}");
        "unknown".into()
    })
}

/// Merge the current thread's `operation_id`/`command`/`source` (see
/// `operation_context`), if any, into `fields`. Values the caller already
/// set (e.g. an event that explicitly names its own `command`) are never
/// overwritten.
///
/// Shared by both local telemetry sinks (`TraceSender::additional_fields`
/// below and `AppInsightsSender::send_event`) so the
/// operation_id/command/source enrichment rule has one
/// implementation instead of being hand-duplicated between them -- a
/// prior version of this function existed independently in each sink,
/// which risked the two silently diverging if the rule ever changed in
/// only one place.
pub(crate) fn merge_operation_context(fields: &mut BTreeMap<String, Value>) {
    if let Some((operation_id, command, source)) = operation_context::current() {
        fields
            .entry(fields::OPERATION_ID.to_string())
            .or_insert_with(|| json!(operation_id));
        fields
            .entry(fields::COMMAND.to_string())
            .or_insert_with(|| json!(command.as_str()));
        fields
            .entry(fields::SOURCE.to_string())
            .or_insert_with(|| json!(source.as_str()));
    }
}

fn populate_additional_fields() -> BTreeMap<String, Value> {
    // TODO: Add more additional fields here as needed
    let mut additional_fields = BTreeMap::new();
    additional_fields.insert(fields::TRIDENT_VERSION.to_string(), json!(TRIDENT_VERSION));
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
    platform_info.insert(
        fields::ASSET_ID.to_string(),
        json!(product_uuid_or_unknown(dmi::read_product_uuid_raw())),
    );
    platform_info.insert(fields::OS_RELEASE.to_string(), json!(get_os_release()));
    platform_info.insert(fields::TOTAL_CPU.to_string(), json!(sys.cpus().len()));
    platform_info.insert(
        fields::TOTAL_MEMORY_GIB.to_string(),
        json!((sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0)).round() as u64),
    );

    let kernel_release = uname::kernel_release().unwrap_or_else(|e| {
        warn!(
            "Failed to get kernel release, using 'unknown' as value: {}",
            e
        );
        "unknown".to_string()
    });
    platform_info.insert(
        fields::KERNEL_VERSION.to_string(),
        json!(kernel_release.trim()),
    );
    platform_info
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{BufRead, BufReader};

    use tracing_subscriber::{filter, layer::SubscriberExt};

    #[test]
    fn test_operation_context_does_not_invent_an_installation_id() {
        operation_context::run_with_operation(
            &CommandKind::update(),
            operation_context::OperationSource::Cli,
            || {
                let mut fields = BTreeMap::new();
                merge_operation_context(&mut fields);
                assert!(fields.contains_key("operation_id"));
                assert!(!fields.contains_key("installation_id"));

                fields.insert("installation_id".to_string(), json!("persisted-id"));
                merge_operation_context(&mut fields);
                assert_eq!(fields["installation_id"], json!("persisted-id"));
            },
        );
    }

    #[test]
    fn test_tracestream() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender =
            tracestream.make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap());
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
    fn test_lock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let mut tracestream = TraceStream::default();
        let trace_sender =
            tracestream.make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap());

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
    fn test_read_product_uuid_unknown() {
        let uuid = product_uuid_or_unknown(Err(anyhow!("UUID file unavailable")));
        assert_eq!(uuid, "unknown");
    }

    #[test]
    fn test_read_product_uuid_exists() {
        let uuid = product_uuid_or_unknown(Ok("test_uuid".to_string()));
        assert_eq!(uuid, "test_uuid");
    }

    #[test]
    fn test_read_product_uuid_preserves_case() {
        // Regression test: must not canonicalize/lowercase a well-formed
        // uppercase UUID, unlike OsUuid's Display.
        let uuid = product_uuid_or_unknown(Ok("6BA7B810-9DAD-11D1-80B4-00C04FD430C8".to_string()));
        assert_eq!(uuid, "6BA7B810-9DAD-11D1-80B4-00C04FD430C8");
    }

    #[test]
    fn test_tracestream_write_metric_event_to_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap())
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
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap())
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
    fn test_tracestream_write_span_metric_to_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap())
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

    #[test]
    /// Same cross-invocation join gap as
    /// `test_servicing_id_stage_then_read_back_across_separate_tracestreams`,
    /// but for `installation_id`: every existing installation_id test
    /// either only exercises the datastore layer (`datastore.rs`) or sets
    /// the value directly on a single `TraceStream`
    /// (`test_tracestream_installation_id_written_to_additional_fields`) --
    /// none of them prove that a *separate* `TraceStream` calling
    /// `attach_ids_if_present` actually recovers an installation ID
    /// created by a different one.
    fn test_installation_id_stage_then_read_back_across_separate_tracestreams() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("db.sqlite");

        // Stage: a fresh TraceStream + datastore, as if this were
        // `Trident::install` creating the installation ID for the first
        // time.
        let mut staging_datastore = DataStore::open_or_create(&db_path).unwrap();
        let staging_tracestream = TraceStream::default();
        operation_context::run_with_operation(
            &CommandKind::install(),
            operation_context::OperationSource::Cli,
            || {
                staging_tracestream
                    .ensure_and_attach_installation_id(&mut staging_datastore)
                    .unwrap();
            },
        );
        let staged_id = staging_tracestream
            .installation_id_handle()
            .read()
            .unwrap()
            .clone()
            .expect("staging call should have created an installation ID");

        // Read back: a completely separate TraceStream, reading only from
        // disk (via the datastore path, matching the real
        // attach_ids_if_present call site signature), as if this were a
        // later command or daemon request in the same process
        // reattaching to an already-provisioned host.
        let readback_tracestream = TraceStream::default();
        readback_tracestream.attach_ids_if_present(&db_path, false);

        assert_eq!(
            readback_tracestream
                .installation_id_handle()
                .read()
                .unwrap()
                .clone(),
            Some(staged_id),
            "a separate TraceStream reading the same datastore should recover the installation ID"
        );
    }

    #[test]
    /// Negative counterpart: a `TraceStream` that only ever reads (never
    /// creates) against a datastore that has never had an installation ID
    /// created, and is not otherwise provisioned, must leave the handle
    /// `None` rather than fabricating one.
    fn test_installation_id_absent_when_never_created() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("db.sqlite");

        // Create the datastore file itself (so attach_ids_if_present's
        // `datastore_path.exists()` check passes), but never create an
        // installation ID on it.
        DataStore::open_or_create(&db_path).unwrap();

        let tracestream = TraceStream::default();
        tracestream.attach_ids_if_present(&db_path, false);

        assert_eq!(
            tracestream.installation_id_handle().read().unwrap().clone(),
            None,
            "installation ID must stay absent when none has ever been created on this datastore"
        );
    }
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
        expected_platform_info.insert(
            "asset_id".to_string(),
            json!(product_uuid_or_unknown(dmi::read_product_uuid_raw())),
        );
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
