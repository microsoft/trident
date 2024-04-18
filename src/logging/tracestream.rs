use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use anyhow::Context;
use chrono::{DateTime, Utc};
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Layer, registry::LookupSpan};

// TODO: Set the constant value based on the current run
const ASSET_ID: &str = "testing-asset-id";

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
    pub asset_id: String,
    pub metric_name: String,
    pub value: Value,
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
}

/// The TraceSender is a struct that holds the server URL and a reqwest client
/// to send the trace entries to the server. It implements the Layer trait from
/// the tracing-subscriber crate to handle the events and send them to the
/// server.
impl TraceSender {
    fn new(server: Arc<RwLock<Option<String>>>) -> Self {
        Self {
            server,
            client: reqwest::blocking::Client::new(),
        }
    }

    fn has_server(&self) -> bool {
        self.server.read().map(|s| s.is_some()).unwrap_or_default()
    }

    fn get_server(&self) -> Option<String> {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
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
        metadata.level() <= &tracing::Level::INFO && self.has_server()
    }

    /// Each time an event is fired, this function is called for the TraceSender
    /// layer to handle the event and send it to the server. It is called only
    /// if enabled returns true. It creates a TraceEntry from the event based on
    /// the information cared about and sends it to the server.
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if let Some(target) = self.get_server() {
            let mut visitor = TraceEntryVisitor::default();
            event.record(&mut visitor);
            let entry = TraceEntry {
                timestamp: Utc::now(),
                // TODO: This needs to be set based on the asset ID of the
                // current run
                asset_id: ASSET_ID.to_string(),
                metric_name: visitor
                    .fields
                    .get("metric_name")
                    //TODO: figure out how to make it required to have a
                    //metric_name for now
                    .map_or("unknown_metric".to_string(), |v| {
                        v.as_str().unwrap_or_default().to_string()
                    }),
                value: visitor.fields.get("value").cloned().unwrap_or_default(),
            };

            let body = match serde_json::to_string(&entry) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Failed to serialize trace entry: {}", e);
                    return;
                }
            };

            if let Err(e) = self.client.post(target).body(body).send() {
                eprintln!("Failed to send trace entry: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracestream() {
        let tracestream = TraceStream::default();
        let trace_sender = tracestream.make_trace_sender();

        assert!(
            !trace_sender.has_server(),
            "tracestream should not have a server"
        );
        assert!(
            trace_sender.get_server().is_none(),
            "tracestream should not have a server"
        );

        tracestream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        assert!(
            trace_sender.has_server(),
            "tracestream should have a server"
        );
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
            !trace_sender.has_server(),
            "tracestream should not have a server"
        );
        assert!(
            trace_sender.get_server().is_none(),
            "tracestream should not have a server"
        );

        tracestream.disable();

        tracestream
            .set_server("http://localhost:8080".to_string())
            .unwrap();

        assert!(
            !trace_sender.has_server(),
            "tracestream should not have a server"
        );
        assert!(
            trace_sender.get_server().is_none(),
            "tracestream should not have a server"
        );
    }
}
