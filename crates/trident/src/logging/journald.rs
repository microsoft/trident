use crate::telemetry::{events, fields};
use std::{fmt, io};

use log::{kv, Record};
use systemd_journal_logger::JournalLog;
use tracing::{
    field::{Field, Visit},
    span, Event, Metadata, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use super::tracestream::TraceStream;

pub struct JournaldTracing {
    tracestream: TraceStream,
    send: Box<dyn Fn(JournalEntry) + Send + Sync>,
}

impl JournaldTracing {
    pub fn new(tracestream: TraceStream) -> io::Result<Self> {
        let journal = JournalLog::new()?.with_syslog_identifier("trident-tracing".to_string());
        Ok(Self {
            tracestream,
            send: Box::new(move |entry| entry.send(&journal)),
        })
    }
}

struct JournalEntry {
    metadata: &'static Metadata<'static>,
    message: String,
    fields: Vec<(String, String)>,
}

impl JournalEntry {
    fn send(&self, journal: &JournalLog) {
        let level = match *self.metadata.level() {
            tracing::Level::ERROR => log::Level::Error,
            tracing::Level::WARN => log::Level::Warn,
            tracing::Level::INFO => log::Level::Info,
            tracing::Level::DEBUG => log::Level::Debug,
            tracing::Level::TRACE => log::Level::Trace,
        };
        let _ = journal.journal_send(
            &Record::builder()
                .args(format_args!("{}", self.message))
                .level(level)
                .target(self.metadata.target())
                .module_path(self.metadata.module_path())
                .file(self.metadata.file())
                .line(self.metadata.line())
                .key_values(self)
                .build(),
        );
    }
}

impl kv::Source for JournalEntry {
    fn visit<'kvs>(&'kvs self, visitor: &mut dyn kv::VisitSource<'kvs>) -> Result<(), kv::Error> {
        for (name, value) in &self.fields {
            visitor.visit_pair(kv::Key::from_str(name), kv::Value::from(value.as_str()))?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct JournalFields(Vec<(String, String)>);

impl JournalFields {
    fn push(&mut self, field: &Field, value: String) {
        self.0.push((field.name().to_string(), value));
    }

    fn extend_entry(&self, fields: &mut Vec<(String, String)>) {
        fields.extend(
            self.0
                .iter()
                .map(|(name, value)| (journal_field_name(name), value.clone())),
        );
    }
}

impl Visit for JournalFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push(field, format!("{value:?}"));
    }
}

fn journal_field_name(name: &str) -> String {
    format!("T_{name}")
        .bytes()
        .map(|c| if c == b'.' { b'_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == b'_')
        .map(|c| char::from(c.to_ascii_uppercase()))
        .collect()
}

impl<S> Layer<S> for JournaldTracing
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut fields = JournalFields::default();
            attrs.record(&mut fields);
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if let Some(fields) = span.extensions_mut().get_mut::<JournalFields>() {
                values.record(fields);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Vec::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                let metadata = span.metadata();
                fields.push(("SPAN_NAME".to_string(), span.name().to_string()));
                fields.push(("SPAN_TARGET".to_string(), metadata.target().to_string()));
                if let Some(file) = metadata.file() {
                    fields.push(("SPAN_CODE_FILE".to_string(), file.to_string()));
                }
                if let Some(line) = metadata.line() {
                    fields.push(("SPAN_CODE_LINE".to_string(), line.to_string()));
                }
                if let Some(span_fields) = span.extensions().get::<JournalFields>() {
                    span_fields.extend_entry(&mut fields);
                }
            }
        }

        let mut event_fields = JournalFields::default();
        event.record(&mut event_fields);
        let metric_name = event_fields
            .0
            .iter()
            .find(|(name, _)| name == fields::METRIC_NAME)
            .map(|(_, value)| value.as_str())
            .unwrap_or_default()
            .to_string();
        let correlation = self.tracestream.correlation_fields(&metric_name);
        let message = event_fields
            .0
            .iter()
            .find(|(name, _)| name == "message")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        event_fields.0.retain(|(name, _)| name != "message");
        event_fields.extend_entry(&mut fields);

        for (name, value) in correlation {
            let name = journal_field_name(&name);
            fields.retain(|(key, _)| key != &name);
            let value = value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string());
            fields.push((name, value));
        }
        if matches!(
            metric_name.as_str(),
            events::COMMAND_START | events::TRIDENT_START
        ) {
            let persistent_ids = [
                journal_field_name(fields::INSTALLATION_ID),
                journal_field_name(fields::SERVICING_ID),
            ];
            fields.retain(|(name, _)| !persistent_ids.contains(name));
        }

        (self.send)(JournalEntry {
            metadata: event.metadata(),
            message,
            fields,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, Registry};

    use crate::{
        command_kind::CommandKind,
        logging::operation_context::{self, OperationSource},
    };

    use super::*;

    fn capture(tracestream: TraceStream) -> (JournaldTracing, Arc<Mutex<Vec<JournalEntry>>>) {
        let entries = Arc::new(Mutex::new(Vec::new()));
        let output = entries.clone();
        (
            JournaldTracing {
                tracestream,
                send: Box::new(move |entry| output.lock().unwrap().push(entry)),
            },
            entries,
        )
    }

    fn field<'a>(entry: &'a JournalEntry, name: &str) -> Option<&'a str> {
        entry
            .fields
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn correlates_command_events_and_suppresses_startup_persistent_ids() {
        for (source, expected_source) in [
            OperationSource::Cli,
            OperationSource::Daemon,
            OperationSource::GrpcClient,
        ]
        .into_iter()
        .zip(["cli", "daemon", "grpc-client"])
        {
            for command in [CommandKind::install(), CommandKind::commit()] {
                let stream = TraceStream::default();
                if !matches!(source, OperationSource::GrpcClient) {
                    stream.set_installation_id("installation".to_string());
                    stream.set_servicing_id("previous-servicing".to_string());
                }
                let directory = tempfile::tempdir().unwrap();
                let metrics_path = directory.path().join("metrics.jsonl");
                let sender = stream
                    .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true);
                let (layer, entries) = capture(stream);
                let _guard =
                    tracing::subscriber::set_default(Registry::default().with(sender).with(layer));
                let operation_id = operation_context::run_with_operation(&command, source, || {
                    tracing::info!(metric_name = "trident_start", value = 2);
                    tracing::info!(metric_name = "work", value = true);
                    operation_context::current().unwrap().0
                });
                let entries = entries.lock().unwrap();
                assert_eq!(entries.len(), 3);
                let jsonl = std::fs::read_to_string(metrics_path).unwrap();
                for (entry, line) in entries.iter().zip(jsonl.lines()) {
                    let metric: serde_json::Value = serde_json::from_str(line).unwrap();
                    for name in [
                        "operation_id",
                        "installation_id",
                        "servicing_id",
                        "command",
                        "source",
                    ] {
                        assert_eq!(
                            field(entry, &journal_field_name(name)),
                            metric["additional_fields"][name].as_str(),
                        );
                    }
                }
                assert_eq!(jsonl.lines().count(), entries.len());
                for (entry, name) in entries
                    .iter()
                    .zip(["command_start", "trident_start", "work"])
                {
                    assert_eq!(field(entry, "T_METRIC_NAME"), Some(name));
                    assert_eq!(field(entry, "T_OPERATION_ID"), Some(operation_id.as_str()));
                    assert_eq!(field(entry, "T_COMMAND"), Some(command.as_str()));
                    assert_eq!(field(entry, "T_SOURCE"), Some(expected_source));
                    if name != "work" {
                        assert_eq!(field(entry, "T_INSTALLATION_ID"), None);
                        assert_eq!(field(entry, "T_SERVICING_ID"), None);
                    } else if !matches!(source, OperationSource::GrpcClient) {
                        assert_eq!(field(entry, "T_INSTALLATION_ID"), Some("installation"));
                        let servicing_id = if command.generates_servicing_id() {
                            operation_id.as_str()
                        } else {
                            "previous-servicing"
                        };
                        assert_eq!(field(entry, "T_SERVICING_ID"), Some(servicing_id));
                    }
                }
            }
        }
    }

    #[test]
    fn refreshes_identity_per_event_and_isolates_daemon_requests() {
        let stream = TraceStream::default();
        let (layer, entries) = capture(stream.clone());
        let _guard = tracing::subscriber::set_default(Registry::default().with(layer));
        let span = tracing::info_span!("request");
        let _entered = span.enter();
        let mut operation_ids = Vec::new();
        for id in ["first", "second"] {
            stream.set_installation_id(id.to_string());
            stream.set_servicing_id(id.to_string());
            operation_ids.push(operation_context::run_with_operation(
                &CommandKind::commit(),
                OperationSource::Daemon,
                || {
                    tracing::info!(metric_name = "work");
                    operation_context::current().unwrap().0
                },
            ));
        }
        *stream.servicing_id_handle().write().unwrap() = None;
        tracing::info!(metric_name = "background");
        let entries = entries.lock().unwrap();
        assert_ne!(operation_ids[0], operation_ids[1]);
        for (index, id) in ["first", "second"].into_iter().enumerate() {
            let entry = &entries[index * 2 + 1];
            assert_eq!(
                field(entry, "T_OPERATION_ID"),
                Some(operation_ids[index].as_str())
            );
            assert_eq!(field(entry, "T_INSTALLATION_ID"), Some(id));
            assert_eq!(field(entry, "T_SERVICING_ID"), Some(id));
        }
        assert_eq!(field(&entries[4], "T_OPERATION_ID"), None);
        assert_eq!(field(&entries[4], "T_SERVICING_ID"), None);
        assert_eq!(field(&entries[4], "T_INSTALLATION_ID"), Some("second"));
    }

    #[test]
    fn preserves_event_and_span_fields_across_reentry_and_recording() {
        let (layer, entries) = capture(TraceStream::default());
        let _guard = tracing::subscriber::set_default(
            Registry::default().with(layer.with_filter(LevelFilter::INFO)),
        );
        let outer = tracing::info_span!("outer", phase = tracing::field::Empty, "net.bytes" = 7);
        let inner = tracing::info_span!(parent: &outer, "inner", enabled = true);
        for phase in ["first", "second"] {
            outer.record("phase", phase);
            let _entered = inner.enter();
            tracing::debug!("filtered");
            tracing::info!(target: "journal_test", value = 2.5, label = "plain", "line1\nline2");
        }
        let entries = entries.lock().unwrap();
        assert_eq!(entries.len(), 2);
        for (entry, phase) in entries.iter().zip(["first", "second"]) {
            assert_eq!(entry.message, "line1\nline2");
            assert_eq!(entry.metadata.target(), "journal_test");
            assert_eq!(entry.metadata.level(), &tracing::Level::INFO);
            assert!(entry.metadata.file().unwrap().ends_with("journald.rs"));
            assert!(entry.metadata.line().is_some());
            let names: Vec<_> = entry
                .fields
                .iter()
                .filter(|(key, _)| key == "SPAN_NAME")
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(names, ["outer", "inner"]);
            assert_eq!(field(entry, "T_PHASE"), Some(phase));
            assert_eq!(field(entry, "T_NET_BYTES"), Some("7"));
            assert_eq!(field(entry, "T_ENABLED"), Some("true"));
            assert_eq!(field(entry, "T_VALUE"), Some("2.5"));
            assert_eq!(field(entry, "T_LABEL"), Some("plain"));
            assert_eq!(field(entry, "T_OPERATION_ID"), None);
        }
    }

    #[test]
    fn unscoped_events_do_not_invent_identity_and_startup_cannot_leak_explicit_ids() {
        let (layer, entries) = capture(TraceStream::default());
        let _guard = tracing::subscriber::set_default(Registry::default().with(layer));
        tracing::info!(metric_name = "work");
        let span = tracing::info_span!(
            "startup",
            installation_id = "span-installation",
            servicing_id = "span-servicing"
        );
        let _entered = span.enter();
        tracing::info!(
            metric_name = "trident_start",
            installation_id = "event-installation",
            servicing_id = "event-servicing"
        );
        let entries = entries.lock().unwrap();
        for entry in entries.iter() {
            for name in [
                "T_OPERATION_ID",
                "T_INSTALLATION_ID",
                "T_SERVICING_ID",
                "T_COMMAND",
                "T_SOURCE",
            ] {
                assert_eq!(field(entry, name), None);
            }
        }
    }
}
