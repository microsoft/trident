use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    sync::{Arc, RwLock},
    time::Instant,
};

use anyhow::Context;
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

use osutils::{
    files,
    osrelease::{OsRelease, OS_RELEASE_PATH},
    uname,
};

use crate::{TRIDENT_METRICS_FILE_PATH, TRIDENT_VERSION};

/// The product uuid is used to identify the hardware that Trident is running on.
const PRODUCT_UUID_FILE: &str = "/sys/class/dmi/id/product_uuid";
lazy_static::lazy_static! {
    static ref ADDITIONAL_FIELDS: BTreeMap<String, Value> = populate_additional_fields();
    static ref PLATFORM_INFO: BTreeMap<String, Value> = populate_platform_info();
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

#[derive(Default)]
pub struct TraceStream {
    // TODO: Consider changing this to a LockOnce when rustc is updated to
    // >=1.70
    target: Arc<RwLock<Option<String>>>,
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

    pub fn set_server(&self, url: String) -> Result<(), anyhow::Error> {
        if self.disabled {
            info!("tracestream is disabled, ignoring set_server");
            return Ok(());
        }

        reqwest::Url::parse(&url).context(format!("Failed to parse tracestream URL: {}", url))?;
        let mut val = self
            .target
            .write()
            .map_err(|_| anyhow::anyhow!("Failed to lock tracestream"))?;
        val.replace(url);
        Ok(())
    }

    /// Create a Boxed TraceSender
    pub fn make_trace_sender(&self) -> Box<TraceSender> {
        Box::new(TraceSender::new(self.target.clone()))
    }
}

pub struct TraceSender {
    server: Arc<RwLock<Option<String>>>,
    client: reqwest::blocking::Client,
    metrics_file: Option<File>,
}

struct ExecutionTime(Instant);

/// The TraceSender is a struct that holds the server URL and a reqwest client
/// to send the trace entries to the server. It implements the Layer trait from
/// the tracing-subscriber crate to handle the events and send them to the
/// server.
impl TraceSender {
    fn new(server: Arc<RwLock<Option<String>>>) -> Self {
        Self {
            server,
            client: reqwest::blocking::Client::new(),
            metrics_file: match files::create_file(TRIDENT_METRICS_FILE_PATH) {
                Ok(f) => Some(f),
                Err(err) => {
                    eprintln!(
                        "Tracestream setup error: failed to create local metrics file: {:?}",
                        err
                    );
                    None
                }
            },
        }
    }

    fn get_server(&self) -> Option<String> {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
    }

    fn write_metric_to_file(&self, metric: String) {
        if let Some(mut file) = self.metrics_file.as_ref() {
            if let Err(e) = file.write_all(format!("{}\n", metric).as_bytes()) {
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
            additional_fields: ADDITIONAL_FIELDS.clone(),
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
            additional_fields: ADDITIONAL_FIELDS.clone(),
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

/// Obtain product uuid of the hardware Trident is running on
fn read_product_uuid(filepath: String) -> String {
    match fs::read_to_string(filepath.clone()) {
        Ok(uuid) => uuid.trim().to_string(),
        Err(_) => {
            debug!("Failed to read product uuid from {}", filepath);
            "unknown".into()
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
    platform_info.insert(
        "asset_id".to_string(),
        json!(read_product_uuid(PRODUCT_UUID_FILE.into())),
    );
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

    use std::{fs::File, io::Write};

    #[test]
    fn test_tracestream() {
        let tracestream = TraceStream::default();
        let trace_sender = tracestream.make_trace_sender();
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
        let mut tracestream = TraceStream::default();
        let trace_sender = tracestream.make_trace_sender();

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
        let uuid = read_product_uuid("unknown".to_string());
        assert_eq!(uuid, "unknown");
    }

    #[test]
    fn test_read_product_uuid_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let filepath = temp_dir.path().join("product_uuid");
        let mut file = File::create(&filepath).unwrap();
        file.write_all("test_uuid".as_bytes()).unwrap();
        let uuid = read_product_uuid(filepath.to_str().unwrap().to_string());
        assert_eq!(uuid, "test_uuid");
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use std::io::{BufRead, BufReader};

    use super::*;

    use pytest_gen::functional_test;
    use tracing_subscriber::{filter, layer::SubscriberExt};

    #[functional_test]
    fn test_tracestream_write_metric_event_to_file() {
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender()
            .with_filter(filter::LevelFilter::INFO);

        tracing::subscriber::set_global_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        )
        .context("Failed to set global default subscriber")
        .unwrap();

        tracing::info!(metric_name = "test_metric", value = true);

        // Ensure the trace system has time to write the file.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check if the specific metric exists in the file.
        let file = File::open(TRIDENT_METRICS_FILE_PATH).unwrap();
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
            json!(read_product_uuid(PRODUCT_UUID_FILE.into())),
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

    #[functional_test]
    fn test_tracestream_write_span_metric_to_file() {
        let tracestream = TraceStream::default();
        let trace_sender = tracestream
            .make_trace_sender()
            .with_filter(filter::LevelFilter::INFO);

        tracing::subscriber::set_global_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        )
        .context("Failed to set global default subscriber")
        .unwrap();

        // Call test function that will create a span
        simulate_function_span();

        // Ensure the trace system has time to simulate a span.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check if the specific metric exists in the file.
        let file = File::open(TRIDENT_METRICS_FILE_PATH).unwrap();
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
