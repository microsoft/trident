//! Best-effort tracing sink that forwards Trident's metric/span tracing
//! events to Azure Monitor / Application Insights.
//!
//! This intentionally does not depend on the OpenTelemetry SDK or an
//! Application Insights client crate. It follows the same minimal approach
//! as [`super::tracestream::TraceSender`]: parse the Application Insights
//! *connection string* (`InstrumentationKey=<k>;IngestionEndpoint=<url>;...`)
//! ourselves and build the raw Application Insights `EventData` envelope.
//!
//! Sending is delegated to the same [`super::background_uploader`] used by
//! [`super::logstream::Logstream`]: `send_event` only enqueues the envelope
//! and returns immediately, so tracing-layer callbacks (which run on
//! whichever thread emitted the event) are never blocked on network I/O.
//! The background uploader performs the actual `POST` to
//! `${ingestion_endpoint}/v2/track` with a short, bounded timeout on its own
//! dedicated thread. Failures (enqueue, network, non-2xx, etc.) are
//! logged and otherwise swallowed -- telemetry must never be able to affect
//! servicing outcomes.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use log::trace;
use serde_json::{json, Value};
use tracing::{
    field::{Field, Visit},
    span, Event, Subscriber,
};
use tracing_subscriber::{layer::Layer, registry::LookupSpan};
use url::Url;

use super::{background_uploader::BackgroundUploadHandle, tracestream::PLATFORM_INFO};
use crate::TRIDENT_VERSION;

/// Default Application Insights ingestion endpoint, used when the connection
/// string does not specify one explicitly.
const DEFAULT_INGESTION_ENDPOINT: &str = "https://dc.services.visualstudio.com";

/// Per-request total timeout, enforced by the background uploader. Telemetry
/// must never meaningfully delay Trident's actual work.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// `Content-Type` for the Application Insights ingestion request.
const CONTENT_TYPE_JSON: &str = "application/json";

/// A parsed Application Insights connection string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnParts {
    /// Ingestion endpoint, trailing slash stripped (e.g.
    /// `https://region.in.applicationinsights.azure.com`).
    pub ingestion_endpoint: String,
    /// Instrumentation key.
    pub instrumentation_key: String,
}

impl ConnParts {
    /// The `POST` target: `${ingestion_endpoint}/v2/track`.
    fn track_url(&self) -> Option<Url> {
        Url::parse(&format!(
            "{}/v2/track",
            self.ingestion_endpoint.trim_end_matches('/')
        ))
        .ok()
    }
}

/// Parse an Application Insights connection string of the form
/// `InstrumentationKey=<k>;IngestionEndpoint=https://...;...`. Returns `None`
/// if the string is empty, unparsable, or missing an instrumentation key. A
/// missing ingestion endpoint falls back to the public Application Insights
/// endpoint.
pub(crate) fn parse_connection_string(s: &str) -> Option<ConnParts> {
    let mut instrumentation_key: Option<String> = None;
    let mut ingestion_endpoint: Option<String> = None;

    for part in s.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "instrumentationkey" => instrumentation_key = Some(value.trim().to_string()),
            "ingestionendpoint" => {
                ingestion_endpoint = Some(value.trim().trim_end_matches('/').to_string())
            }
            _ => {}
        }
    }

    let instrumentation_key = instrumentation_key.filter(|k| !k.is_empty())?;
    Some(ConnParts {
        ingestion_endpoint: ingestion_endpoint
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| DEFAULT_INGESTION_ENDPOINT.to_string()),
        instrumentation_key,
    })
}

/// A visitor that records the fields of a tracing event/span as a
/// `BTreeMap`, mirroring [`super::tracestream::TraceEntryVisitor`].
#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, Value>,
}

impl Visit for FieldVisitor {
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
            .insert(field.name().to_string(), json!(format!("{value:?}")));
    }
}

/// Timestamp recorded when a span is entered, used to compute execution time
/// on exit.
struct SpanStart(Instant);

/// Renders a JSON value as a string, since Application Insights `EventData`
/// properties are a `Map<string, string>`.
fn stringify(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A `tracing_subscriber::Layer` that forwards Trident's metric events and
/// instrumented spans to Application Insights, best-effort.
///
/// Only constructed when a connection string was compiled into the binary
/// (see [`crate::AZURE_MONITOR_CONNECTION_STRING`]) *and* telemetry has been
/// enabled via the Agent Configuration file (see
/// [`crate::agentconfig::AgentConfig::telemetry_enabled`]); see
/// [`AppInsightsSender::from_connection_string`].
pub struct AppInsightsSender {
    instrumentation_key: String,
    track_url: Url,
    uploader: BackgroundUploadHandle,
}

impl AppInsightsSender {
    /// Build a sender from an Application Insights connection string.
    /// Returns `None` if the string is empty, fails to parse, or the
    /// ingestion endpoint does not form a valid URL.
    pub fn from_connection_string(
        connection_string: &str,
        uploader: BackgroundUploadHandle,
    ) -> Option<Self> {
        let parts = parse_connection_string(connection_string)?;
        Self::from_parts(parts, uploader)
    }

    fn from_parts(parts: ConnParts, uploader: BackgroundUploadHandle) -> Option<Self> {
        let track_url = parts.track_url()?;
        Some(Self {
            instrumentation_key: parts.instrumentation_key,
            track_url,
            uploader,
        })
    }

    /// Build and enqueue an Application Insights `EventData` envelope for
    /// the background uploader to send, best-effort. This only serializes
    /// the envelope and hands it off to the uploader's channel, so it never
    /// blocks on network I/O. A serialization failure or a closed uploader
    /// is logged here at `trace` level; a later network error or non-2xx
    /// response is logged by the background uploader itself (at `error`
    /// level, same as any other background upload). Either way the failure
    /// is otherwise ignored -- it can never affect servicing outcomes.
    fn send_event(&self, name: &str, mut properties: BTreeMap<String, Value>) {
        properties.insert("trident_version".to_string(), json!(TRIDENT_VERSION));
        for (key, value) in PLATFORM_INFO.iter() {
            properties.insert(key.clone(), json!(stringify(value)));
        }

        let string_properties: BTreeMap<String, String> = properties
            .into_iter()
            .map(|(key, value)| (key, stringify(&value)))
            .collect();

        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let envelope = json!({
            "name": "Microsoft.ApplicationInsights.Event",
            "time": now,
            "iKey": self.instrumentation_key,
            "tags": { "ai.internal.sdkVersion": format!("trident:{TRIDENT_VERSION}") },
            "data": {
                "baseType": "EventData",
                "baseData": {
                    "ver": 2,
                    "name": name,
                    "properties": string_properties,
                }
            }
        });

        let body = match serde_json::to_vec(&envelope) {
            Ok(b) => b,
            Err(e) => {
                trace!("Failed to serialize Application Insights event: {e}");
                return;
            }
        };

        if let Err(e) = self.uploader.upload(
            &self.track_url,
            body,
            REQUEST_TIMEOUT,
            Some(CONTENT_TYPE_JSON),
        ) {
            trace!("Failed to enqueue Application Insights event: {e}");
        }
    }
}

/// The `Layer` implementation mirrors
/// [`super::tracestream::TraceSender`]'s event/span handling, but renders an
/// Application Insights `EventData` envelope instead of Trident's own
/// metrics-file format.
impl<S> Layer<S> for AppInsightsSender
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        metadata.level() <= &tracing::Level::INFO
    }

    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let Some(metric_name) = visitor
            .fields
            .get("metric_name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            // Not a metric event (e.g. a plain log line); nothing to forward.
            return;
        };

        let properties: BTreeMap<String, Value> = visitor
            .fields
            .into_iter()
            .filter(|(key, _)| key != "metric_name")
            .collect();

        self.send_event(&metric_name, properties);
    }

    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        id: &span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut visitor = FieldVisitor::default();
            attrs.record(&mut visitor);
            span.extensions_mut().insert(visitor);
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanStart(Instant::now()));
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let Some(SpanStart(start)) = span.extensions_mut().remove::<SpanStart>() else {
            return;
        };
        let Some(mut visitor) = span.extensions_mut().remove::<FieldVisitor>() else {
            return;
        };

        visitor.fields.insert(
            "execution_time".to_string(),
            json!(start.elapsed().as_secs_f64()),
        );

        self.send_event(span.name(), visitor.fields);
    }

    fn on_record(
        &self,
        id: &span::Id,
        values: &span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            if let Some(visitor) = span.extensions_mut().get_mut::<FieldVisitor>() {
                values.record(visitor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_connection_string_full() {
        let parts = parse_connection_string(
            "InstrumentationKey=abc123;IngestionEndpoint=https://region.example/;LiveEndpoint=https://live.example/",
        )
        .expect("should parse");
        assert_eq!(parts.instrumentation_key, "abc123");
        assert_eq!(parts.ingestion_endpoint, "https://region.example");
        assert_eq!(
            parts.track_url(),
            Some(Url::parse("https://region.example/v2/track").unwrap())
        );
    }

    #[test]
    fn test_parse_connection_string_missing_endpoint_uses_default() {
        let parts = parse_connection_string("InstrumentationKey=abc123").expect("should parse");
        assert_eq!(parts.instrumentation_key, "abc123");
        assert_eq!(parts.ingestion_endpoint, DEFAULT_INGESTION_ENDPOINT);
        assert_eq!(
            parts.track_url(),
            Some(Url::parse(&format!("{DEFAULT_INGESTION_ENDPOINT}/v2/track")).unwrap())
        );
    }

    #[test]
    fn test_parse_connection_string_missing_ikey_is_none() {
        assert!(parse_connection_string("IngestionEndpoint=https://region.example/").is_none());
        assert!(parse_connection_string(
            "InstrumentationKey=;IngestionEndpoint=https://region.example/"
        )
        .is_none());
    }

    #[test]
    fn test_parse_connection_string_empty_is_none() {
        assert!(parse_connection_string("").is_none());
    }

    #[test]
    fn test_from_connection_string_empty_is_none() {
        assert!(
            AppInsightsSender::from_connection_string("", BackgroundUploadHandle::new_mock())
                .is_none()
        );
    }

    #[test]
    fn test_from_connection_string_builds_sender() {
        let sender = AppInsightsSender::from_connection_string(
            "InstrumentationKey=k;IngestionEndpoint=https://region.example/",
            BackgroundUploadHandle::new_mock(),
        )
        .expect("should build sender");
        assert_eq!(sender.instrumentation_key, "k");
        assert_eq!(
            sender.track_url,
            Url::parse("https://region.example/v2/track").unwrap()
        );
    }

    #[test]
    fn test_stringify() {
        assert_eq!(stringify(&json!("hello")), "hello");
        assert_eq!(stringify(&json!(42)), "42");
        assert_eq!(stringify(&json!(true)), "true");
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{io::Read, net::TcpListener, sync::mpsc::channel};

    use pytest_gen::functional_test;
    use tracing_subscriber::{filter, layer::SubscriberExt};

    /// Spins up a local TCP listener standing in for the Application
    /// Insights ingestion endpoint, and confirms the sender actually posts a
    /// well-formed `EventData` envelope to it over the network.
    #[functional_test]
    fn test_app_insights_sender_posts_event() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let mut received = Vec::new();
                // Best-effort single read; the request is small enough to
                // arrive in one go for this test.
                if let Ok(n) = stream.read(&mut buf) {
                    received.extend_from_slice(&buf[..n]);
                }
                let _ = tx.send(String::from_utf8_lossy(&received).to_string());
            }
        });

        let uploader = crate::BackgroundUploader::new().expect("should build uploader");
        let sender = AppInsightsSender::from_parts(
            ConnParts {
                ingestion_endpoint: format!("http://{addr}"),
                instrumentation_key: "test-key".to_string(),
            },
            uploader.get_handle().expect("uploader should be alive"),
        )
        .expect("should build sender")
        .with_filter(filter::LevelFilter::INFO);

        let _guard =
            tracing::subscriber::set_default(tracing_subscriber::Registry::default().with(sender));

        tracing::info!(metric_name = "test_metric", value = true);

        let request = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|e| panic!("did not receive a request: {e:?}"));

        assert!(request.contains("POST /v2/track"));
        assert!(request.contains("\"name\":\"test_metric\""));
        assert!(request.contains("\"iKey\":\"test-key\""));
    }
}
