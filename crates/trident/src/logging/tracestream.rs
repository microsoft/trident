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
use log::{debug, info, trace, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sysinfo::System;
use tracing::{
    field::{Field, Visit},
    span, Event, Subscriber,
};
use tracing_subscriber::{layer::Layer, registry::LookupSpan};

use trident_api::error::TridentError;

use osutils::{
    osrelease::{OsRelease, OS_RELEASE_PATH},
    uname, virt,
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
    /// Stable for the lifetime of the datastore, unlike `installation_id`
    /// which is tied to a specific `Trident::install` invocation. See
    /// `crate::datastore::DataStore::datastore_id`.
    datastore_id: Arc<RwLock<Option<String>>>,
    /// Identifies one logical servicing operation (an install, update, or
    /// manual rollback) that may span multiple invocations -- a stage
    /// call, a later finalize call, and a later still `commit` call each
    /// get their own `operation_id`, but share this one value. See
    /// `crate::datastore::DataStore::ensure_servicing_id`/`servicing_id`.
    servicing_id: Arc<RwLock<Option<String>>>,
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
    /// (see [`Self::attach_installation_id_if_present`] and
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

    /// Set the database ID to attach to every trace entry sent from this
    /// point forward, as an additional field, so that all traces/metrics
    /// against a given datastore can be correlated. Unlike
    /// `installation_id`, this is stable for the datastore's entire
    /// lifetime, not just a single `Trident::install` invocation. Expected
    /// to be called once the datastore's persisted database ID has been
    /// retrieved (see [`Self::attach_datastore_id_if_present`]).
    pub fn set_datastore_id(&self, datastore_id: String) {
        match self.datastore_id.write() {
            Ok(mut val) => {
                val.replace(datastore_id);
            }
            Err(_) => warn!("Failed to lock tracestream to set database ID"),
        }
    }

    /// Returns a clone of the shared database-ID handle -- the same
    /// underlying `Arc<RwLock<..>>` written by `set_datastore_id` -- so
    /// other telemetry sinks (namely `AppInsightsSender`) can read the
    /// current value at send-time without needing their own copy of the
    /// logic that sets it.
    pub fn datastore_id_handle(&self) -> Arc<RwLock<Option<String>>> {
        self.datastore_id.clone()
    }

    fn datastore_id_cached(&self) -> bool {
        self.datastore_id
            .read()
            .map(|v| v.is_some())
            .unwrap_or(false)
    }

    /// Best-effort attempt to attach this datastore's database ID -- either
    /// already cached from a prior call on this `TraceStream`, or freshly
    /// read from the datastore at `datastore_path` if one already exists
    /// there. Never creates a datastore: unlike `installation_id`, the
    /// database ID's get-or-create semantics only ever run against a
    /// datastore that has already been opened for real (see
    /// `crate::datastore::DataStore::datastore_id`), so it is safe to call
    /// this any time a datastore is known to already exist.
    pub fn attach_datastore_id_if_present(&self, datastore_path: &Path) {
        if self.datastore_id_cached() || !datastore_path.exists() {
            return;
        }
        match DataStore::open(datastore_path).and_then(|mut ds| ds.datastore_id()) {
            Ok(datastore_id) => {
                info!("Datastore ID: {datastore_id}");
                self.set_datastore_id(datastore_id.to_string());
            }
            Err(e) => {
                warn!("Failed to read/create database ID: {e:?}");
            }
        }
    }

    /// Best-effort attempt to attach this host's installation ID -- either
    /// already cached from a prior call on this `TraceStream`, or freshly
    /// read from the datastore at `datastore_path` if one already exists
    /// there. Never creates a *datastore*, and never creates an
    /// installation ID for a genuinely unprovisioned host: a command that
    /// is allowed to initialize a brand-new datastore (see
    /// [`crate::datastore::DataStore::may_initialize_datastore_for_command`])
    /// must still call [`Self::ensure_and_attach_installation_id`] instead,
    /// on a datastore handle it already owns.
    ///
    /// Not fully read-only, though: for a datastore that is already
    /// provisioned (via offline init or the CIH update-bootstrap path,
    /// both of which adopt a datastore without ever calling
    /// `Trident::install`) but has no installation ID yet, this performs a
    /// one-time migration *write* to mint one -- see
    /// [`crate::datastore::DataStore::installation_id_or_migrate`]. Callers
    /// that require true read-only behavior (e.g. a genuinely
    /// unprivileged/diagnostic path) must not assume this call can never
    /// write to the datastore.
    ///
    /// Safe to call from anywhere, any number of times, before any point
    /// that wants the ID attached: this is the single implementation
    /// shared by every read-only-in-the-common-case attach call site (the
    /// CLI's dispatch, the daemon's startup attach, the daemon's
    /// per-request backstop, and `Trident::new`'s own attach), so a
    /// correctness fix to this logic only needs to happen once.
    pub fn attach_installation_id_if_present(&self, datastore_path: &Path) {
        if self.installation_id_cached() || !datastore_path.exists() {
            return;
        }
        match DataStore::open(datastore_path).and_then(|mut ds| ds.installation_id_or_migrate()) {
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

    /// Ensures `datastore` has an installation ID -- creating one if this
    /// is the first access, or reading back the existing one otherwise --
    /// and attaches it. Unlike
    /// [`Self::attach_installation_id_if_present`], this is only for the
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

    /// Set the servicing ID to attach to every trace entry sent from this
    /// point forward. Expected to be called once per `Trident`/`TraceStream`
    /// lifetime, either by [`Self::ensure_and_attach_servicing_id`] (the
    /// call that stages a new servicing operation) or
    /// [`Self::attach_servicing_id_if_present`] (a finalize-only or
    /// `commit` call reading one back).
    pub fn set_servicing_id(&self, servicing_id: String) {
        match self.servicing_id.write() {
            Ok(mut val) => {
                val.replace(servicing_id);
            }
            Err(_) => warn!("Failed to lock tracestream to set servicing ID"),
        }
    }

    /// Returns a clone of the shared servicing-ID handle -- the same
    /// underlying `Arc<RwLock<..>>` written by `set_servicing_id` -- so
    /// other telemetry sinks (namely `AppInsightsSender`) can read the
    /// current value at send-time without needing their own copy of the
    /// logic that sets it.
    pub fn servicing_id_handle(&self) -> Arc<RwLock<Option<String>>> {
        self.servicing_id.clone()
    }

    /// Best-effort attempt to attach the currently-persisted servicing ID
    /// (if any) -- read-only, never generates one. Used by a finalize-only
    /// call (`Operations::has_stage() == false`) and by `commit`, both of
    /// which must correlate with whatever servicing ID an earlier stage
    /// call already persisted, rather than generating their own.
    ///
    /// Deliberately always re-reads from `datastore` rather than
    /// short-circuiting on an already-cached value (unlike
    /// `attach_installation_id_if_present`/`attach_datastore_id_if_present`,
    /// which *are* safe to cache-and-skip): `installation_id`/
    /// `datastore_id` are stable for the entire lifetime of the
    /// `TraceStream`/`DataStore` they're attached to, but `servicing_id`
    /// is not -- a single long-lived daemon `TraceStream` (see
    /// `server::tridentserver`, which owns one `TraceStream` shared across
    /// every request it serves) can legitimately see a *different*
    /// already-staged servicing operation across separate finalize/commit
    /// requests, e.g. one staged out-of-process via the CLI. Caching the
    /// first value read here would silently keep attaching that stale ID
    /// to every later finalize/commit call in the same process, even once
    /// the datastore itself has moved on.
    ///
    /// Deliberately takes an already-open `datastore` handle rather than a
    /// path (unlike `attach_installation_id_if_present`): by the time a
    /// finalize/commit call reaches this point it already holds one, and
    /// a servicing ID is only ever meaningful in the context of a
    /// datastore that has already been through at least one staged
    /// operation.
    pub fn attach_servicing_id_if_present(&self, datastore: &DataStore) {
        match datastore.servicing_id() {
            Ok(Some(servicing_id)) => {
                info!("Servicing ID: {servicing_id}");
                self.set_servicing_id(servicing_id.to_string());
            }
            Ok(None) => {
                debug!("No servicing ID persisted yet (nothing staged on this datastore)");
            }
            Err(e) => {
                warn!("Failed to read servicing ID: {e:?}");
            }
        }
    }

    /// Generates a fresh servicing ID for a new staging operation and
    /// attaches it. Called by `Trident::install`/`update`/`rollback`
    /// exactly when `Operations::has_stage()` is true for that invocation
    /// -- a finalize-only invocation must call
    /// [`Self::attach_servicing_id_if_present`] instead, to read back the
    /// value this call persists rather than generating its own.
    ///
    /// Best-effort: a failure here must not block the servicing operation
    /// itself, matching the "telemetry never affects servicing outcomes"
    /// invariant -- callers should `warn!`-and-continue on error rather
    /// than propagating it with `?`.
    pub fn ensure_and_attach_servicing_id(
        &self,
        datastore: &mut DataStore,
    ) -> Result<(), TridentError> {
        let servicing_id = datastore.ensure_servicing_id()?;
        info!("Servicing ID: {servicing_id}");
        self.set_servicing_id(servicing_id.to_string());
        Ok(())
    }

    /// Generates (if `has_stage` is true, from a source allowed to
    /// generate persistent IDs) or reads back (otherwise) the servicing ID
    /// for the current invocation, and attaches it. Single shared
    /// implementation for the branch used identically by
    /// `Trident::install`/`update`/`rollback` (each passing their own
    /// `Operations::has_stage()`) and `Trident::commit` (which always
    /// passes `has_stage = false`, since `commit` never stages anything
    /// itself) -- see those call sites for the full rationale.
    ///
    /// Reading back is unconditional and source-agnostic -- it is a
    /// harmless, read-only best-effort lookup regardless of who is calling
    /// -- but generating is gated on `should_generate_persistent_ids` and
    /// fails closed (does not generate) if the source is unknown or not
    /// allowed to generate persistent IDs (see that function's doc
    /// comment). Best-effort throughout: a failure to generate is only
    /// logged, matching every other persistent-ID attachment in this
    /// module.
    ///
    /// Deliberately regenerates (overwriting any previously-staged value)
    /// whenever `has_stage` is true, even if a servicing operation is
    /// already staged and this call only ends up finalizing it: any
    /// invocation that is allowed to stage is entitled to a fresh
    /// servicing ID for that possibility.
    pub fn refresh_servicing_id(&self, datastore: &mut DataStore, has_stage: bool) {
        let source = operation_context::current().map(|(_, _, source)| source);
        if has_stage && source.is_some_and(operation_context::should_generate_persistent_ids) {
            if let Err(e) = self.ensure_and_attach_servicing_id(datastore) {
                warn!("Failed to create servicing ID: {e:?}");
            }
        } else {
            self.attach_servicing_id_if_present(datastore);
        }
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
            self.datastore_id.clone(),
            self.servicing_id.clone(),
            metrics_file_path,
            truncate,
        ))
    }
}

pub struct TraceSender {
    server: Arc<RwLock<Option<String>>>,
    installation_id: Arc<RwLock<Option<String>>>,
    datastore_id: Arc<RwLock<Option<String>>>,
    servicing_id: Arc<RwLock<Option<String>>>,
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
        datastore_id: Arc<RwLock<Option<String>>>,
        servicing_id: Arc<RwLock<Option<String>>>,
        metrics_file_path: &str,
        truncate: bool,
    ) -> Self {
        if let Some(parent) = Path::new(metrics_file_path).parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                eprintln!(
                    "Tracestream setup error: failed to create local metrics file's parent directory: {err:?}"
                );
            }
        }
        // Reset any pre-existing content up front when requested, via a
        // separate truncating open, then always keep the real handle in
        // append-only mode: a plain `File::create` (O_TRUNC without
        // O_APPEND) kept open long-term has its own independent,
        // non-advancing write offset, so a concurrent writer to this same
        // path (e.g. `grpc-client`, opened separately in append mode) that
        // extends the file past that offset would have its data
        // overwritten the next time this descriptor writes. Combining
        // `OpenOptions::truncate(true)` with `.append(true)` in one open()
        // call isn't an option: the standard library requires `.write(true)`
        // for truncation, and adding that back defeats the point of
        // append-only semantics for every later write through this handle.
        if truncate {
            if let Err(err) = File::create(metrics_file_path) {
                eprintln!(
                    "Tracestream setup error: failed to truncate local metrics file: {err:?}"
                );
            }
        }
        let metrics_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(metrics_file_path)
            .map_err(Error::from);
        Self {
            server,
            installation_id,
            datastore_id,
            servicing_id,
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
    /// `TraceStream::set_installation_id`), the servicing ID (if one has
    /// been set via `TraceStream::set_servicing_id`), and the current
    /// thread's `operation_id`/`command` (if any, see `operation_context`),
    /// so entries can be correlated back to a specific host installation
    /// and servicing operation.
    ///
    /// Unlike `installation_id`, `servicing_id` has no `operation_id`
    /// fallback in `merge_operation_context`: it should stay absent for
    /// any event fired before the current call's own
    /// `ensure_and_attach_servicing_id`/`attach_servicing_id_if_present`
    /// has actually run (e.g. a read-only command that never touches
    /// servicing at all), rather than appearing to correlate with a
    /// servicing operation that isn't happening.
    ///
    /// `operation_id`/`command` are deliberately merged here rather than
    /// into the metric's own `value` (as scalar/span fields are): mixing
    /// them into `value` would change the established schema for simple
    /// scalar metrics -- e.g. `clean_install_start` would go from
    /// `"value": true` to `"value": {"command": ..., "operation_id": ...,
    /// "value": true}` the moment it ran inside an operation context,
    /// breaking that contract for existing consumers.
    ///
    /// `installation_id` is filled in by two different paths: normally
    /// from the persisted value set via `TraceStream::set_installation_id`
    /// (attached above), but `merge_operation_context` also falls back to
    /// this invocation's own `operation_id` whenever no persisted value
    /// has been attached yet -- e.g. every event fired before a host's
    /// first-ever `install` has actually created the datastore and created
    /// one. See `merge_operation_context` for why that fallback is the
    /// same value `ensure_installation_id` will end up persisting for
    /// that same invocation.
    fn additional_fields(&self) -> BTreeMap<String, Value> {
        let mut fields = ADDITIONAL_FIELDS.clone();
        if let Ok(installation_id) = self.installation_id.read() {
            if let Some(installation_id) = installation_id.as_ref() {
                fields.insert("installation_id".to_string(), json!(installation_id));
            }
        }
        if let Ok(datastore_id) = self.datastore_id.read() {
            if let Some(datastore_id) = datastore_id.as_ref() {
                fields.insert("datastore_id".to_string(), json!(datastore_id));
            }
        }
        if let Ok(servicing_id) = self.servicing_id.read() {
            if let Some(servicing_id) = servicing_id.as_ref() {
                fields.insert("servicing_id".to_string(), json!(servicing_id));
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

/// Merge the current thread's `operation_id`/`command`/`source` (see
/// `operation_context`), if any, into `fields`. Values the caller already
/// set (e.g. an event that explicitly names its own `command`) are never
/// overwritten.
///
/// Shared by both local telemetry sinks (`TraceSender::additional_fields`
/// below and `AppInsightsSender::send_event`) so the
/// operation_id/command/source/installation_id-fallback rule has one
/// implementation instead of being hand-duplicated between them -- a
/// prior version of this function existed independently in each sink,
/// which risked the two silently diverging if the rule ever changed in
/// only one place.
pub(crate) fn merge_operation_context(fields: &mut BTreeMap<String, Value>) {
    if let Some((operation_id, command, source)) = operation_context::current() {
        fields
            .entry("operation_id".to_string())
            .or_insert_with(|| json!(operation_id));
        fields
            .entry("command".to_string())
            .or_insert_with(|| json!(command));
        fields
            .entry("source".to_string())
            .or_insert_with(|| json!(source.as_str()));
        // If no installation ID has been persisted/attached yet (e.g. this
        // is the invocation that is about to create the datastore and
        // create one), fall back to this invocation's own `operation_id` --
        // the same value `DataStore::ensure_installation_id` will persist
        // as the installation ID once the datastore is actually created.
        fields
            .entry("installation_id".to_string())
            .or_insert_with(|| json!(operation_id));
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

    // Whether this host is virtualized (see `osutils::virt` for the
    // detection heuristic and its caveats).
    platform_info.insert("vm".to_string(), json!(virt::is_virtual()));

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
    /// Regression test: `TraceStream::set_datastore_id` must actually
    /// reach the serialized trace entry's `additional_fields.datastore_id`,
    /// independently of installation_id.
    fn test_tracestream_datastore_id_written_to_additional_fields() {
        let temp_dir = tempfile::tempdir().unwrap();
        let metrics_path = temp_dir.path().join("metrics.jsonl");
        let tracestream = TraceStream::default();
        tracestream.set_datastore_id("test-datastore-id".to_string());
        let trace_sender = tracestream
            .make_trace_sender_with_metrics_path(metrics_path.to_str().unwrap(), true)
            .with_filter(filter::LevelFilter::INFO);

        // See test_tracestream_write_metric_event_to_file for why a scoped
        // (not global) default subscriber is used here.
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::Registry::default().with(trace_sender),
        );

        tracing::info!(metric_name = "test_metric_with_datastore_id", value = true);

        // Ensure the trace system has time to write the file.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let file = File::open(&metrics_path).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        let metric_found = lines.iter().any(|line| {
            line.contains(r#""metric_name":"test_metric_with_datastore_id""#)
                && line.contains(r#""datastore_id":"test-datastore-id""#)
        });

        assert!(
            metric_found,
            "Expected metric with datastore_id field not found in the local metrics file"
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

    #[test]
    /// Regression test closing the stage/read-back join gap: a servicing
    /// ID generated by one `TraceStream` (standing in for the process that
    /// staged an install/update/rollback) must be recoverable by a
    /// completely separate, unrelated `TraceStream` instance that only
    /// shares the same on-disk datastore (standing in for a later
    /// finalize/commit request, or a different daemon request in the same
    /// process). Every other servicing_id test either exercises the
    /// datastore layer alone (`datastore.rs`) or injects a pre-built
    /// handle directly (`appinsights.rs`'s functional test) -- neither
    /// proves the real `refresh_servicing_id` -> `DataStore::servicing_id`
    /// join actually works end-to-end.
    fn test_servicing_id_stage_then_read_back_across_separate_tracestreams() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("db.sqlite");

        // Stage: a fresh TraceStream + datastore, as if this were the
        // process running `Trident::install`'s staging half.
        let mut staging_datastore = DataStore::open_or_create(&db_path).unwrap();
        let staging_tracestream = TraceStream::default();
        operation_context::run_with_operation(
            "install",
            operation_context::OperationSource::Cli,
            || {
                staging_tracestream.refresh_servicing_id(&mut staging_datastore, true);
            },
        );
        let staged_id = staging_tracestream
            .servicing_id_handle()
            .read()
            .unwrap()
            .clone()
            .expect("staging call should have generated a servicing ID");

        // Read back: a completely separate TraceStream + DataStore handle
        // opened against the same file, as if this were a later
        // finalize/commit request (or a different daemon request in the
        // same long-lived process).
        let mut readback_datastore = DataStore::open_or_create(&db_path).unwrap();
        let readback_tracestream = TraceStream::default();
        readback_tracestream.refresh_servicing_id(&mut readback_datastore, false);

        assert_eq!(
            readback_tracestream
                .servicing_id_handle()
                .read()
                .unwrap()
                .clone(),
            Some(staged_id),
            "a separate TraceStream reading the same datastore should recover the staged servicing ID"
        );
    }

    #[test]
    /// Negative counterpart to the join test above: a `TraceStream` that
    /// only ever reads (never stages) against a datastore that has never
    /// had anything staged must leave the servicing ID handle `None`,
    /// rather than fabricating one.
    fn test_servicing_id_absent_when_never_staged() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("db.sqlite");

        let mut datastore = DataStore::open_or_create(&db_path).unwrap();
        let tracestream = TraceStream::default();
        tracestream.refresh_servicing_id(&mut datastore, false);

        assert_eq!(
            tracestream.servicing_id_handle().read().unwrap().clone(),
            None,
            "servicing ID must stay absent when nothing has ever staged on this datastore"
        );
    }

    #[test]
    /// Same cross-invocation join gap as
    /// `test_servicing_id_stage_then_read_back_across_separate_tracestreams`,
    /// but for `installation_id`: every existing installation_id test
    /// either only exercises the datastore layer (`datastore.rs`) or sets
    /// the value directly on a single `TraceStream`
    /// (`test_tracestream_installation_id_written_to_additional_fields`) --
    /// none of them prove that a *separate* `TraceStream` calling
    /// `attach_installation_id_if_present` actually recovers an
    /// installation ID created by a different one.
    fn test_installation_id_stage_then_read_back_across_separate_tracestreams() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("db.sqlite");

        // Stage: a fresh TraceStream + datastore, as if this were
        // `Trident::install` creating the installation ID for the first
        // time.
        let mut staging_datastore = DataStore::open_or_create(&db_path).unwrap();
        let staging_tracestream = TraceStream::default();
        operation_context::run_with_operation(
            "install",
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
        // attach_installation_id_if_present call site signature), as if
        // this were a later command or daemon request in the same
        // process reattaching to an already-provisioned host.
        let readback_tracestream = TraceStream::default();
        readback_tracestream.attach_installation_id_if_present(&db_path);

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

        // Create the datastore file itself (so attach_installation_id_if_present's
        // `datastore_path.exists()` check passes), but never create an
        // installation ID on it.
        DataStore::open_or_create(&db_path).unwrap();

        let tracestream = TraceStream::default();
        tracestream.attach_installation_id_if_present(&db_path);

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
        expected_platform_info.insert("os_release".to_string(), json!(get_os_release()));
        expected_platform_info.insert("total_cpu".to_string(), json!(4));
        expected_platform_info.insert("total_memory_gib".to_string(), json!(6));
        expected_platform_info.insert(
            "kernel_version".to_string(),
            json!(uname::kernel_release().unwrap().trim()),
        );
        expected_platform_info.insert("vm".to_string(), json!(virt::is_virtual()));

        // Call the function to get the actual result.
        let platform_info = populate_platform_info();

        // Assert that the actual result matches the expected result.
        assert_eq!(
            platform_info, expected_platform_info,
            "Platform info does not match the expected result"
        );
    }
}
