//! The high-level [`Client`] for talking to a Nebraska server.

use std::{thread, time::Duration};

use log::{debug, trace, warn};
use semver::Version;
use url::Url;

use super::{
    error::NebraskaError,
    event::{ProgressEvent, TerminalEvent},
    id::MachineId,
    transport::{ReqwestTransport, Transport},
    wire::{self, App},
};

/// The outcome of an update check.
///
/// `UpdateInProgress` is a first-class outcome rather than an error because
/// Nebraska returns it on **every** poll between the first progress event and
/// the terminal event; it is expected server behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// No update is available; the instance is up to date.
    UpToDate,

    /// An update is available. Boxed to keep [`CheckOutcome`] small, since the
    /// no-update outcomes are the common case.
    UpdateAvailable(Box<UpdateOffer>),

    /// Nebraska reports an update is already in progress for this instance.
    /// Expected while an update is mid-flight; the caller should keep polling
    /// (or, post-reboot, report completion) rather than treat it as an error.
    UpdateInProgress,
}

/// An offered update: the version and the files that make it up.
///
/// Nebraska renders a package's [extra files](https://github.com/flatcar/nebraska)
/// as additional entries in the manifest, so an update may consist of more than
/// one file: the [`primary`](UpdateOffer::primary) package and, when present,
/// one or more [`extra_files`](UpdateOffer::extra_files). Callers that only need
/// the main artifact use `primary`; extra-file-aware callers also iterate
/// `extra_files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOffer {
    /// The version being offered.
    pub version: Version,

    /// The primary package file (the manifest's first package; always present).
    pub primary: PackageFile,

    /// Additional files attached to the update, in manifest order. Empty when
    /// the update is a single file.
    pub extra_files: Vec<PackageFile>,
}

/// A single file that is part of an [`UpdateOffer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    /// The file name as listed in the manifest.
    pub name: String,

    /// The absolute URL of the file, resolved by joining the response's
    /// `codebase` with [`name`](PackageFile::name). Frequently on a different
    /// host than the Nebraska server itself, which serves only metadata.
    pub url: Url,

    /// The hash of the file as reported by Nebraska, if any.
    ///
    /// **This is a hash of the file, not of any content inside it.** Nebraska
    /// reports a base64-encoded SHA-1 (with an optional SHA-256), for integrity
    /// checking the downloaded artifact. `None` when the manifest carries no
    /// hash for this file, which the protocol permits.
    pub hash: Option<PackageHash>,

    /// The file size in bytes, when reported by Nebraska.
    pub size: Option<u64>,

    /// Whether the manifest marks this file as required.
    pub required: bool,
}

/// The hash(es) of a file, as reported by Nebraska.
///
/// Both values are base64-encoded and hash the *file* (not its contents).
/// Nebraska reports a SHA-1; `sha256` is present only when the file was
/// registered with one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageHash {
    /// Base64-encoded SHA-1 of the file.
    pub sha1: String,

    /// Base64-encoded SHA-256 of the file, when Nebraska provides it.
    pub sha256: Option<String>,
}

/// The bounded exponential-backoff policy used by
/// [`Client::complete_after_reboot`] when retrying transient failures.
///
/// It is intentionally bounded so that a persistently-unreachable server cannot
/// hang startup forever, but generous, because the call it guards must land to
/// avoid permanently wedging the instance.
#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    /// Maximum number of attempts (including the first).
    max_attempts: u32,
    /// Backoff before the second attempt.
    initial_backoff: Duration,
    /// Upper bound on the (doubling) backoff.
    max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(5),
        }
    }
}

/// Runs `op`, retrying while it returns a [retryable](NebraskaError::is_retryable)
/// error, with an exponential backoff bounded by `policy`. Returns the first
/// `Ok`, or the last error once attempts are exhausted or a permanent error is
/// hit. Blocks the current thread between attempts.
fn retry<T>(
    policy: RetryPolicy,
    mut op: impl FnMut() -> Result<T, NebraskaError>,
) -> Result<T, NebraskaError> {
    let mut backoff = policy.initial_backoff;
    let mut attempt = 1;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(e) if e.is_retryable() && attempt < policy.max_attempts => {
                warn!(
                    "retryable Nebraska error (attempt {attempt}/{}): {e}",
                    policy.max_attempts
                );
                thread::sleep(backoff);
                backoff = (backoff * 2).min(policy.max_backoff);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Resolves a package file name against the manifest's `codebase`.
///
/// The codebase is a directory — and normally a different host than Nebraska
/// itself, since Nebraska serves metadata while the artifact lives in a blob
/// store or CDN — but Nebraska stores whatever URL an operator typed and does
/// not normalize it. Relative resolution *replaces* the last path segment when
/// the trailing slash is missing, turning `https://host/packages` + `os.cosi`
/// into `https://host/os.cosi` — a plausible-looking URL pointing at the wrong
/// place. Appending the separator first keeps the codebase a directory,
/// matching how Omaha clients concatenate the two halves.
fn join_file(codebase: &Url, name: &str) -> Option<Url> {
    let mut base = codebase.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join(name).ok()
}

/// Renders a response for logging: the statuses that drive this client's
/// decisions, and nothing else.
///
/// The raw body is deliberately not logged. A manifest's `codebase` and package
/// URLs are frequently pre-signed (an Azure blob SAS token, for instance), so
/// dumping the XML would write a download credential into the log of anyone who
/// turns on trace diagnostics — the same reason the endpoint is
/// [redacted](redacted).
fn summarize(response: &wire::Response) -> String {
    if response.apps.is_empty() {
        return "no apps".to_string();
    }

    response
        .apps
        .iter()
        .map(|app| {
            let events = app
                .events
                .iter()
                .map(|event| event.status.to_string())
                .collect::<Vec<_>>()
                .join(", ");

            let check = match &app.update_check {
                None => String::new(),
                Some(check) => {
                    let manifest = match &check.manifest {
                        None => String::new(),
                        Some(manifest) => format!(
                            ", manifest {} with {} package(s)",
                            manifest.version,
                            manifest
                                .packages
                                .as_ref()
                                .map_or(0, |packages| packages.packages.len())
                        ),
                    };
                    format!(", updatecheck {}{manifest}", check.status)
                }
            };

            format!(
                "app '{}' status {}, {} event(s) [{events}]{check}",
                app.app_id,
                app.status,
                app.events.len()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Renders an endpoint for logging with everything after the authority removed.
///
/// A Nebraska endpoint can carry an Omaha secret (Nebraska answers `501` when
/// the configured secret is missing from the client's URL), so only the scheme
/// and host are safe to emit; the path, query, fragment, and any userinfo are
/// dropped rather than selectively scrubbed, so a secret cannot leak from a
/// component this code did not anticipate.
fn redacted(endpoint: &Url) -> String {
    let Some(host) = endpoint.host_str() else {
        return "<redacted>".to_string();
    };
    let port = endpoint.port().map(|p| format!(":{p}")).unwrap_or_default();
    format!("{}://{host}{port}/<redacted>", endpoint.scheme())
}

/// A client for a single Nebraska app on a single track.
///
/// The client bundles the immutable request identity — endpoint, app id,
/// `track`, and [`MachineId`]. Because these are required to construct the
/// client and every request flows through it, two protocol invariants hold
/// structurally: `track` is present on every request including event-only ones
/// (Nebraska resolves the group from `track` before processing events, so an
/// omitted track silently drops them), and the machine id is always a validated,
/// unbraced value (Nebraska hides braced ids from its UI and statistics).
///
/// # Event ordering and the all-or-nothing rule
///
/// Emitting a [progress event](Client::report_progress) is a **commitment** to
/// eventually emit a terminal event: leaving an instance in a progress state
/// wedges it permanently, because Nebraska's self-heal path only triggers from
/// the `UpdateGranted` state and nothing resets instance status on a timer. The
/// terminal event is sent *after the reboot* — i.e. from a different process —
/// so this cannot be enforced at compile time; instead the terminal operations
/// are exposed as dedicated, hard-to-forget methods
/// ([`complete_after_reboot`](Client::complete_after_reboot) and
/// [`report_failure`](Client::report_failure)), and the caller is responsible
/// for persisting enough state across the reboot to make that call.
///
/// A client that sends **no** events at all is always safe: Nebraska self-heals
/// the instance to Complete on the next check at the new version. Prefer that
/// over sending a partial sequence.
pub struct Client<T: Transport = ReqwestTransport> {
    endpoint: Url,
    app_id: String,
    track: String,
    machine_id: MachineId,
    client_version: String,
    transport: T,
}

impl Client<ReqwestTransport> {
    /// Creates a client using the default blocking `reqwest` transport.
    ///
    /// `endpoint` should be the Nebraska update URL (typically ending in
    /// `/v1/update/`, with the trailing slash preserved).
    ///
    /// `client_version` identifies the **updater** — the program driving the
    /// update — in `<request version>`, which Nebraska surfaces in its UI and
    /// logs. It is the caller's to supply: this module speaks the protocol and
    /// has no version of its own worth reporting to an operator.
    pub fn new(
        endpoint: Url,
        app_id: impl Into<String>,
        track: impl Into<String>,
        machine_id: MachineId,
        client_version: impl Into<String>,
    ) -> Self {
        Self::with_transport(
            endpoint,
            app_id,
            track,
            machine_id,
            client_version,
            ReqwestTransport::new(),
        )
    }
}

impl<T: Transport> Client<T> {
    /// Creates a client with an explicit [`Transport`], primarily for testing.
    pub fn with_transport(
        endpoint: Url,
        app_id: impl Into<String>,
        track: impl Into<String>,
        machine_id: MachineId,
        client_version: impl Into<String>,
        transport: T,
    ) -> Self {
        Self {
            endpoint,
            app_id: app_id.into(),
            track: track.into(),
            machine_id,
            client_version: client_version.into(),
            transport,
        }
    }

    /// Checks for an available update, reporting `current_version` as the
    /// instance's current version.
    ///
    /// `current_version` **must be the real version** and valid semver: a client
    /// reporting `0.0.0` is offered an update on every poll forever, and a
    /// non-semver version fails instance registration server-side.
    pub fn check_for_update(
        &self,
        current_version: &Version,
    ) -> Result<CheckOutcome, NebraskaError> {
        let app = self.app(current_version).with_update_check();
        let response = self.send(app)?;
        self.interpret_check(response)
    }

    /// Reports a [`ProgressEvent`] for an in-flight update.
    ///
    /// Only valid after a successful [`check_for_update`](Client::check_for_update)
    /// has caused Nebraska to grant the update: Nebraska rejects events from an
    /// instance it has never seen. Emitting a progress event commits the caller
    /// to eventually reporting a terminal event — see the [type docs](Client).
    pub fn report_progress(
        &self,
        current_version: &Version,
        event: ProgressEvent,
    ) -> Result<(), NebraskaError> {
        let app = self.app(current_version).with_event(event.wire());
        let response = self.send(app)?;
        self.require_events_accepted(&response, 1)?;
        Ok(())
    }

    /// Reports successful completion after the reboot, retrying transient
    /// failures automatically.
    ///
    /// This sends the single batched request Nebraska expects: a terminal
    /// `complete` event plus a `<ping/>` plus an `<updatecheck/>`. Nebraska
    /// processes the event before the update check within one request, so this
    /// both moves the instance to Complete and returns a clean `noupdate` in one
    /// round trip — closing the window in which a bare post-reboot poll would hit
    /// `error-updateInProgressOnInstance`. This is the terminal event that
    /// discharges the commitment made by [`report_progress`](Client::report_progress).
    ///
    /// # Why this retries by default
    ///
    /// The first network call immediately after a reboot routinely fails while
    /// DNS and routing settle, and **losing this terminal event wedges the
    /// instance permanently** — there is no server-side self-heal, timer, or REST
    /// reset. Retrying is therefore a correctness requirement, not a
    /// quality-of-service choice, so it is the default here: this method retries
    /// [retryable](NebraskaError::is_retryable) failures with a bounded
    /// exponential backoff and returns the last error only once transient
    /// retries are exhausted or a permanent error occurs.
    ///
    /// A response that still reports the update as in progress means the
    /// completion did not take, so it is retried too, and surfaces as
    /// [`NebraskaError::CompletionNotAcknowledged`] if it never lands — never as
    /// a successful outcome.
    ///
    /// The retry window is bounded by both the per-request timeout of the
    /// [transport](crate::nebraska::ReqwestTransport) and this method's attempt
    /// count and backoff; with the defaults of each, it gives up after under two
    /// minutes rather than blocking indefinitely.
    ///
    /// This blocks the calling thread while retrying. A caller that has its own
    /// scheduler (e.g. an existing poll loop) and would rather re-attempt on its
    /// own cadence should use [`try_complete_after_reboot`](Client::try_complete_after_reboot)
    /// instead and drive the retry itself. If completion genuinely cannot be
    /// reported, see [`report_failure`](Client::report_failure) for the recovery
    /// path.
    ///
    /// `previous_version` is the version the instance was on before the update;
    /// `current_version` is the (new) version now running.
    pub fn complete_after_reboot(
        &self,
        previous_version: &Version,
        current_version: &Version,
    ) -> Result<CheckOutcome, NebraskaError> {
        self.complete_after_reboot_with_policy(
            RetryPolicy::default(),
            previous_version,
            current_version,
        )
    }

    /// [`complete_after_reboot`](Client::complete_after_reboot) with an explicit
    /// retry policy, so the retry behaviour can be exercised without waiting out
    /// the production backoff.
    fn complete_after_reboot_with_policy(
        &self,
        policy: RetryPolicy,
        previous_version: &Version,
        current_version: &Version,
    ) -> Result<CheckOutcome, NebraskaError> {
        retry(policy, || {
            match self.try_complete_after_reboot(previous_version, current_version)? {
                // Still in progress means the terminal event did not take.
                // Convert it into a retryable error so the bounded retry
                // re-sends it, rather than reporting success for an instance
                // that is still wedged.
                CheckOutcome::UpdateInProgress => Err(NebraskaError::CompletionNotAcknowledged),
                outcome => Ok(outcome),
            }
        })
    }

    /// Reports successful completion after the reboot in a **single attempt**,
    /// without retrying.
    ///
    /// This is the non-retrying variant of
    /// [`complete_after_reboot`](Client::complete_after_reboot); prefer that
    /// method unless you are driving retries yourself.
    ///
    /// Because losing the terminal event wedges the instance permanently, a
    /// caller using this variant **must** retry the call itself while the
    /// returned error [is retryable](NebraskaError::is_retryable) *or* the
    /// outcome is [`CheckOutcome::UpdateInProgress`] (with a bounded backoff),
    /// giving up only on a permanent error. See
    /// [`complete_after_reboot`](Client::complete_after_reboot) for the rationale
    /// and [`report_failure`](Client::report_failure) for the recovery path.
    pub fn try_complete_after_reboot(
        &self,
        previous_version: &Version,
        current_version: &Version,
    ) -> Result<CheckOutcome, NebraskaError> {
        let app = self
            .app(current_version)
            .with_event_from_version(
                TerminalEvent::Completed.wire(),
                previous_version.to_string(),
            )
            .with_ping()
            .with_update_check();
        let response = self.send(app)?;

        // Landing the terminal event is what this request exists for, and a
        // dropped event is the only failure here that can wedge the instance —
        // so confirm delivery before interpreting the update check that rode
        // along with it.
        self.require_events_accepted(&response, 1)?;

        // The instance should now be Complete; a still-in-progress status means
        // the completion did not take and the caller should retry.
        match self.interpret_check(response) {
            // The event was acknowledged, so the completion *was* recorded and
            // the instance is not wedged; the batched update check simply could
            // not grant — a rollout policy, a throttled or disabled group. Both
            // stages report through the same app status, so without this the
            // most safety-critical call in the API would report a permanent
            // failure for an update that succeeded.
            Err(NebraskaError::ServerError(status)) => {
                debug!("completion recorded; the batched update check reported '{status}'");
                Ok(CheckOutcome::UpToDate)
            }
            result => result,
        }
    }

    /// Reports a failed update (terminal `3/0`), which moves the instance to
    /// Error, clears `update_in_progress`, and re-arms it so a subsequent check
    /// can grant again. This is the "reset and retry" path for a wedged or
    /// failed update.
    pub fn report_failure(
        &self,
        previous_version: &Version,
        current_version: &Version,
    ) -> Result<(), NebraskaError> {
        let app = self
            .app(current_version)
            .with_event_from_version(TerminalEvent::Failed.wire(), previous_version.to_string());
        let response = self.send(app)?;
        self.require_events_accepted(&response, 1)?;
        Ok(())
    }

    /// Builds the base `<app>` for a request, carrying the client identity.
    fn app(&self, version: &Version) -> App {
        App::new(
            self.app_id.clone(),
            version.to_string(),
            self.track.clone(),
            self.machine_id.to_string(),
        )
    }

    /// Serializes, sends, and parses a request/response round-trip.
    fn send(&self, app: App) -> Result<wire::Response, NebraskaError> {
        let request = wire::request_for(app, self.client_version.clone());
        let body = request
            .to_xml()
            .map_err(|e| NebraskaError::Serialize(e.to_string()))?;
        trace!(
            "Nebraska request to '{}':\n{}",
            redacted(&self.endpoint),
            String::from_utf8_lossy(&body)
        );
        let text = self.transport.post_xml(&self.endpoint, &body)?;
        trace!("Nebraska response: {} bytes", text.len());
        let response = wire::parse_response(&text).map_err(NebraskaError::Parse)?;
        trace!("Nebraska response: {}", summarize(&response));
        Ok(response)
    }

    /// Locates this client's app in a response, validating it is present and
    /// that its id matches.
    fn app_response<'r>(
        &self,
        response: &'r wire::Response,
    ) -> Result<&'r wire::AppResponse, NebraskaError> {
        match response.apps.as_slice() {
            [] => Err(NebraskaError::UnexpectedResponse(
                "response contained no app".to_string(),
            )),
            [app] => {
                if app.app_id != self.app_id {
                    return Err(NebraskaError::UnexpectedResponse(format!(
                        "response app id '{}' does not match requested '{}'",
                        app.app_id, self.app_id
                    )));
                }
                Ok(app)
            }
            apps => Err(NebraskaError::UnexpectedResponse(format!(
                "expected exactly one app in response, found {}",
                apps.len()
            ))),
        }
    }

    /// Validates that the server recorded the `expected` events sent with this
    /// request.
    ///
    /// Delivery is judged by the **per-event acknowledgements**, not by the app
    /// status. Nebraska resolves the app and the group (from `track`) *before*
    /// processing events and, when either lookup fails, sets an app-level error
    /// status and returns early — so the events are dropped while the response
    /// is still HTTP 200 and carries no `<event>` element at all. Once the
    /// events *are* processed, an acknowledgement is emitted for each one, and
    /// any later app-level error belongs to the update check that rode along in
    /// the same request rather than to the events. Judging delivery by the app
    /// status alone would therefore conflate a lost event with a rollout policy
    /// that merely declined to grant an update.
    fn require_events_accepted(
        &self,
        response: &wire::Response,
        expected: usize,
    ) -> Result<(), NebraskaError> {
        let app = self.app_response(response)?;

        // A rejected event, for servers that report event failures this way.
        // Nebraska always answers `ok`, so this only ever fires against another
        // Omaha implementation.
        if let Some(rejected) = app.events.iter().find(|event| !event.status.is_ok()) {
            return Err(NebraskaError::ServerError(format!(
                "event rejected with status '{}'",
                rejected.status
            )));
        }

        if app.events.len() >= expected {
            return Ok(());
        }

        // Unacknowledged events: report the server's own explanation when it
        // gave one, since that names the misconfiguration (an unknown app id or
        // track) that caused the drop.
        if !app.status.is_ok() && !app.status.is_update_in_progress() {
            return Err(NebraskaError::ServerError(app.status.to_string()));
        }

        Err(NebraskaError::UnexpectedResponse(format!(
            "Nebraska acknowledged {} of {expected} events",
            app.events.len()
        )))
    }

    /// Interprets a response that carries an update check into a [`CheckOutcome`].
    fn interpret_check(&self, response: wire::Response) -> Result<CheckOutcome, NebraskaError> {
        let app = self.app_response(&response)?;

        if app.status.is_update_in_progress() {
            debug!("Nebraska reports an update already in progress for this instance");
            return Ok(CheckOutcome::UpdateInProgress);
        }

        if !app.status.is_ok() {
            return Err(NebraskaError::ServerError(app.status.to_string()));
        }

        let update_check = app.update_check.as_ref().ok_or_else(|| {
            NebraskaError::UnexpectedResponse("app response missing updatecheck".to_string())
        })?;

        if update_check.status.is_no_update() {
            return Ok(CheckOutcome::UpToDate);
        }

        if !update_check.status.is_update_available() {
            return Err(NebraskaError::ServerError(update_check.status.to_string()));
        }

        let offer = self.build_offer(update_check)?;
        Ok(CheckOutcome::UpdateAvailable(Box::new(offer)))
    }

    /// Builds an [`UpdateOffer`] from a positive update-check response.
    fn build_offer(
        &self,
        update_check: &wire::UpdateCheckResponse,
    ) -> Result<UpdateOffer, NebraskaError> {
        let manifest = update_check.manifest.as_ref().ok_or_else(|| {
            NebraskaError::UnexpectedResponse("update available but no manifest".to_string())
        })?;

        let version = Version::parse(&manifest.version).map_err(|e| {
            NebraskaError::UnexpectedResponse(format!(
                "offered version '{}' is not valid semver: {e}",
                manifest.version
            ))
        })?;

        let codebase = update_check
            .urls
            .as_ref()
            .and_then(|u| u.urls.first())
            .ok_or_else(|| {
                NebraskaError::UnexpectedResponse(
                    "update available but no codebase URL".to_string(),
                )
            })?;

        let packages = manifest
            .packages
            .as_ref()
            .map(|p| p.packages.as_slice())
            .unwrap_or(&[]);

        // Nebraska renders extra files as additional packages after the main
        // one, so the manifest may legitimately carry more than one. The first
        // is the primary file; any following are extra files.
        let (primary, extras) = packages.split_first().ok_or_else(|| {
            NebraskaError::UnexpectedResponse("update available but no package listed".to_string())
        })?;

        let primary = self.build_package_file(&codebase.codebase, primary)?;
        let extra_files = extras
            .iter()
            .map(|p| self.build_package_file(&codebase.codebase, p))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(UpdateOffer {
            version,
            primary,
            extra_files,
        })
    }

    /// Builds a [`PackageFile`] from a single manifest package, resolving its URL
    /// against `codebase`.
    fn build_package_file(
        &self,
        codebase: &Url,
        package: &wire::Package,
    ) -> Result<PackageFile, NebraskaError> {
        let url = join_file(codebase, &package.name).ok_or_else(|| {
            NebraskaError::UnexpectedResponse(format!(
                "failed to resolve package '{}' against codebase '{}'",
                package.name,
                redacted(codebase)
            ))
        })?;

        let hash = package.hash.as_ref().map(|sha1| PackageHash {
            sha1: sha1.clone(),
            sha256: package.hash_sha256.clone(),
        });

        // Size is a string on the wire, and the attribute is not optional there:
        // a package registered without a size is serialized as `size="0"`, which
        // means "unknown" rather than an empty file. Treat that — and an
        // unparseable value — as absent rather than failing the offer.
        let size = package
            .size
            .as_ref()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|size| *size != 0);

        Ok(PackageFile {
            name: package.name.clone(),
            url,
            hash,
            size,
            required: package.required,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// Stands in for the updater version a real caller would supply.
    const TEST_CLIENT_VERSION: &str = "test-updater-1.2.3";

    /// A canned transport: records the last request body and returns a fixed
    /// response, so client logic is exercised without a network.
    struct MockTransport {
        response: String,
        last_body: RefCell<Option<String>>,
    }

    impl MockTransport {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
                last_body: RefCell::new(None),
            }
        }
    }

    impl Transport for MockTransport {
        fn post_xml(&self, _endpoint: &Url, body: &[u8]) -> Result<String, NebraskaError> {
            *self.last_body.borrow_mut() = Some(String::from_utf8_lossy(body).into_owned());
            Ok(self.response.clone())
        }
    }

    fn client_with(response: &str) -> Client<MockTransport> {
        Client::with_transport(
            Url::parse("https://nebraska.example/v1/update/").unwrap(),
            "app-1",
            "stable",
            MachineId::new("mid-1").unwrap(),
            TEST_CLIENT_VERSION,
            MockTransport::new(response),
        )
    }

    /// A transport that replays a script of responses, repeating the last one
    /// once the script is exhausted, so a retry loop can be driven through a
    /// sequence of server answers.
    struct ScriptedTransport {
        responses: Vec<String>,
        calls: Cell<u32>,
    }

    impl Transport for ScriptedTransport {
        fn post_xml(&self, _endpoint: &Url, _body: &[u8]) -> Result<String, NebraskaError> {
            let index = (self.calls.get() as usize).min(self.responses.len() - 1);
            self.calls.set(self.calls.get() + 1);
            Ok(self.responses[index].clone())
        }
    }

    fn client_with_responses(responses: Vec<&str>) -> Client<ScriptedTransport> {
        Client::with_transport(
            Url::parse("https://nebraska.example/v1/update/").unwrap(),
            "app-1",
            "stable",
            MachineId::new("mid-1").unwrap(),
            TEST_CLIENT_VERSION,
            ScriptedTransport {
                responses: responses.into_iter().map(String::from).collect(),
                calls: Cell::new(0),
            },
        )
    }

    /// A completion response that reports the instance as still updating.
    const IN_PROGRESS: &str = r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-updateInProgressOnInstance"><event status="ok"/><updatecheck status="error-internal"/></app></response>"#;

    /// A completion response that reports the instance as settled and current.
    const NO_UPDATE: &str = r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><event status="ok"/><updatecheck status="noupdate"/></app></response>"#;

    const OFFER: &str = r#"
        <response protocol="3.0" server="nebraska">
          <daystart elapsed_seconds="0"/>
          <app appid="app-1" status="ok">
            <updatecheck status="ok">
              <urls><url codebase="https://updates.example.com/"/></urls>
              <manifest version="2.0.0">
                <packages><package name="os-image-2.0.0.cosi" hash="AAAAAAAAAAAAAAAAAAAAAAAAAAA=" size="1024" required="true"/></packages>
              </manifest>
            </updatecheck>
          </app>
        </response>"#;

    #[test]
    fn check_returns_offer_with_joined_url() {
        let client = client_with(OFFER);
        let outcome = client.check_for_update(&Version::new(1, 0, 0)).unwrap();
        match outcome {
            CheckOutcome::UpdateAvailable(offer) => {
                assert_eq!(offer.version, Version::new(2, 0, 0));
                assert!(offer.extra_files.is_empty());
                assert_eq!(offer.primary.name, "os-image-2.0.0.cosi");
                assert_eq!(
                    offer.primary.url.as_str(),
                    "https://updates.example.com/os-image-2.0.0.cosi"
                );
                assert_eq!(
                    offer.primary.hash,
                    Some(PackageHash {
                        sha1: "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
                        sha256: None,
                    })
                );
                assert_eq!(offer.primary.size, Some(1024));
                assert!(offer.primary.required);
            }
            other => panic!("expected an update offer, got {other:?}"),
        }
    }

    #[test]
    fn check_offer_without_hash_is_none() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="ok"><urls><url codebase="https://updates.example.com/"/></urls><manifest version="2.0.0"><packages><package name="x.cosi" required="true"/></packages></manifest></updatecheck></app></response>"#,
        );
        match client.check_for_update(&Version::new(1, 0, 0)).unwrap() {
            CheckOutcome::UpdateAvailable(offer) => {
                assert_eq!(offer.primary.hash, None);
                assert_eq!(offer.primary.size, None);
            }
            other => panic!("expected an offer, got {other:?}"),
        }
    }

    #[test]
    fn check_offer_zero_size_is_none() {
        // `size` is not optional on the wire, so a package registered without
        // one arrives as "0", meaning unknown rather than an empty file.
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="ok"><urls><url codebase="https://updates.example.com/"/></urls><manifest version="2.0.0"><packages><package name="x.cosi" size="0" required="true"/></packages></manifest></updatecheck></app></response>"#,
        );
        match client.check_for_update(&Version::new(1, 0, 0)).unwrap() {
            CheckOutcome::UpdateAvailable(offer) => assert_eq!(offer.primary.size, None),
            other => panic!("expected an offer, got {other:?}"),
        }
    }

    #[test]
    fn check_offer_codebase_without_trailing_slash_keeps_directory() {
        // Relative resolution would replace the last segment and yield
        // https://updates.example.com/x.cosi, silently pointing elsewhere.
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="ok"><urls><url codebase="https://updates.example.com/packages"/></urls><manifest version="2.0.0"><packages><package name="x.cosi" required="true"/></packages></manifest></updatecheck></app></response>"#,
        );
        match client.check_for_update(&Version::new(1, 0, 0)).unwrap() {
            CheckOutcome::UpdateAvailable(offer) => assert_eq!(
                offer.primary.url.as_str(),
                "https://updates.example.com/packages/x.cosi"
            ),
            other => panic!("expected an offer, got {other:?}"),
        }
    }

    #[test]
    fn check_offer_surfaces_extra_files() {
        // Nebraska renders extra files as additional <package> entries after the
        // main one; the offer exposes them as extra_files in order.
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="ok"><urls><url codebase="https://updates.example.com/"/></urls><manifest version="2.0.0"><packages>
              <package name="os-image-2.0.0.cosi" hash="MAIN" size="10" required="true"/>
              <package name="os-image-2.0.0.sig" hash="SIG1" hash_sha256="SIG256" size="20" required="false"/>
              <package name="notes.txt" size="5" required="false"/>
            </packages></manifest></updatecheck></app></response>"#,
        );
        match client.check_for_update(&Version::new(1, 0, 0)).unwrap() {
            CheckOutcome::UpdateAvailable(offer) => {
                assert_eq!(offer.primary.name, "os-image-2.0.0.cosi");
                assert_eq!(offer.extra_files.len(), 2);

                let sig = &offer.extra_files[0];
                assert_eq!(sig.name, "os-image-2.0.0.sig");
                assert_eq!(
                    sig.url.as_str(),
                    "https://updates.example.com/os-image-2.0.0.sig"
                );
                assert_eq!(
                    sig.hash,
                    Some(PackageHash {
                        sha1: "SIG1".to_string(),
                        sha256: Some("SIG256".to_string()),
                    })
                );
                assert_eq!(sig.size, Some(20));
                assert!(!sig.required);

                let notes = &offer.extra_files[1];
                assert_eq!(notes.name, "notes.txt");
                assert_eq!(notes.hash, None);
            }
            other => panic!("expected an offer, got {other:?}"),
        }
    }

    #[test]
    fn check_reports_no_update() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="noupdate"/></app></response>"#,
        );
        assert_eq!(
            client.check_for_update(&Version::new(2, 0, 0)).unwrap(),
            CheckOutcome::UpToDate
        );
    }

    #[test]
    fn check_maps_update_in_progress() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-updateInProgressOnInstance"><updatecheck status="error-internal"/></app></response>"#,
        );
        assert_eq!(
            client.check_for_update(&Version::new(2, 0, 0)).unwrap(),
            CheckOutcome::UpdateInProgress
        );
    }

    #[test]
    fn check_wrong_app_id_is_error() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="other" status="ok"><updatecheck status="noupdate"/></app></response>"#,
        );
        let err = client.check_for_update(&Version::new(1, 0, 0)).unwrap_err();
        assert!(
            matches!(err, NebraskaError::UnexpectedResponse(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn report_progress_sends_track_and_event() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><event status="ok"/></app></response>"#,
        );
        client
            .report_progress(&Version::new(1, 0, 0), ProgressEvent::DownloadStarted)
            .unwrap();
        let body = client.transport.last_body.borrow().clone().unwrap();
        assert!(body.contains(r#"track="stable""#), "{body}");
        assert!(
            body.contains(r#"<event eventtype="13" eventresult="1""#),
            "{body}"
        );
    }

    #[test]
    fn report_progress_fails_when_app_status_is_an_error() {
        // Nebraska sets an app-level error and returns *before* processing
        // events when it cannot resolve the app or the track, so the events are
        // silently dropped on an HTTP 200. That must not look like success.
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-failedToRetrieveUpdatePackageInfo"><updatecheck status="error-internal"/></app></response>"#,
        );
        let err = client
            .report_progress(&Version::new(1, 0, 0), ProgressEvent::DownloadStarted)
            .unwrap_err();
        assert!(matches!(err, NebraskaError::ServerError(_)), "got {err:?}");
    }

    #[test]
    fn report_progress_tolerates_update_in_progress_status() {
        // Progress events are sent precisely while an update is in flight, so
        // this status says nothing about whether the event was accepted.
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-updateInProgressOnInstance"><event status="ok"/></app></response>"#,
        );
        client
            .report_progress(&Version::new(1, 0, 0), ProgressEvent::DownloadStarted)
            .unwrap();
    }

    #[test]
    fn report_failure_rejected_event_is_an_error() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><event status="error-internal"/></app></response>"#,
        );
        let err = client
            .report_failure(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap_err();
        assert!(matches!(err, NebraskaError::ServerError(_)), "got {err:?}");
    }

    #[test]
    fn complete_after_reboot_batches_event_ping_and_check() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><event status="ok"/><updatecheck status="noupdate"/></app></response>"#,
        );
        let outcome = client
            .complete_after_reboot(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap();
        assert_eq!(outcome, CheckOutcome::UpToDate);
        let body = client.transport.last_body.borrow().clone().unwrap();
        assert!(
            body.contains(r#"<event eventtype="3" eventresult="2""#),
            "{body}"
        );
        assert!(body.contains(r#"previousversion="1.0.0""#), "{body}");
        assert!(body.contains("<ping"), "{body}");
        assert!(body.contains("<updatecheck"), "{body}");
    }

    #[test]
    fn complete_after_reboot_retries_while_update_in_progress() {
        // The completion did not take on the first attempt; the retry must
        // re-send it rather than reporting success for a still-wedged instance.
        let client = client_with_responses(vec![IN_PROGRESS, NO_UPDATE]);
        let outcome = client
            .complete_after_reboot_with_policy(
                fast_policy(),
                &Version::new(1, 0, 0),
                &Version::new(2, 0, 0),
            )
            .unwrap();
        assert_eq!(outcome, CheckOutcome::UpToDate);
        assert_eq!(client.transport.calls.get(), 2);
    }

    #[test]
    fn complete_after_reboot_fails_when_completion_never_lands() {
        let client = client_with_responses(vec![IN_PROGRESS]);
        let err = client
            .complete_after_reboot_with_policy(
                fast_policy(),
                &Version::new(1, 0, 0),
                &Version::new(2, 0, 0),
            )
            .unwrap_err();
        assert!(
            matches!(err, NebraskaError::CompletionNotAcknowledged),
            "got {err:?}"
        );
        assert_eq!(client.transport.calls.get(), fast_policy().max_attempts);
    }

    #[test]
    fn complete_after_reboot_succeeds_when_only_the_batched_check_is_declined() {
        // The event was acknowledged, so the completion landed; the app-level
        // error belongs to the update check that rode along (a throttled or
        // disabled rollout). Reporting that as a failure would send the caller
        // down the recovery path for an update that actually succeeded.
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-maxUpdatesPerPeriodLimitReached"><event status="ok"/><updatecheck status="error-internal"/></app></response>"#,
        );
        let outcome = client
            .complete_after_reboot(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap();
        assert_eq!(outcome, CheckOutcome::UpToDate);
    }

    #[test]
    fn complete_after_reboot_fails_when_the_event_was_dropped() {
        // No <event> acknowledgement: Nebraska could not resolve the app or the
        // track and returned before processing events, so the terminal event was
        // lost even though the app carries an error status.
        let client = client_with_responses(vec![
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-failedToRetrieveUpdatePackageInfo"><updatecheck status="error-internal"/></app></response>"#,
        ]);
        let err = client
            .complete_after_reboot_with_policy(
                fast_policy(),
                &Version::new(1, 0, 0),
                &Version::new(2, 0, 0),
            )
            .unwrap_err();
        assert!(matches!(err, NebraskaError::ServerError(_)), "got {err:?}");
    }

    #[test]
    fn complete_after_reboot_sends_previous_version_on_the_event() {
        let client = client_with(NO_UPDATE);
        client
            .complete_after_reboot(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap();
        let body = client.transport.last_body.borrow().clone().unwrap();
        assert!(
            body.contains(r#"<event eventtype="3" eventresult="2" previousversion="1.0.0""#),
            "{body}"
        );
    }

    #[test]
    fn join_file_accepts_an_absolute_name() {
        // The manifest may point at a wholly different host: Nebraska serves
        // metadata, while the artifact usually lives in a blob store or CDN.
        let codebase = Url::parse("https://updates.example.com/packages/").unwrap();
        assert_eq!(
            join_file(&codebase, "https://cdn.example.net/os.cosi")
                .unwrap()
                .as_str(),
            "https://cdn.example.net/os.cosi"
        );
    }

    #[test]
    fn join_file_accepts_names_under_the_codebase() {
        let codebase = Url::parse("https://updates.example.com/packages/").unwrap();
        assert_eq!(
            join_file(&codebase, "os.cosi").unwrap().as_str(),
            "https://updates.example.com/packages/os.cosi"
        );
        // A name may still address a subdirectory of the codebase.
        assert_eq!(
            join_file(&codebase, "2.0.0/os.cosi").unwrap().as_str(),
            "https://updates.example.com/packages/2.0.0/os.cosi"
        );
    }

    #[test]
    fn summarize_reports_statuses_without_urls() {
        // The traced summary must not carry the manifest URLs, which are
        // routinely pre-signed.
        let response = wire::parse_response(OFFER).unwrap();
        let summary = summarize(&response);
        assert!(summary.contains("app 'app-1' status ok"), "{summary}");
        assert!(summary.contains("updatecheck ok"), "{summary}");
        assert!(
            summary.contains("manifest 2.0.0 with 1 package(s)"),
            "{summary}"
        );
        assert!(!summary.contains("updates.example.com"), "{summary}");
    }

    #[test]
    fn redacted_endpoint_drops_path_and_query() {
        let url = Url::parse("https://nebraska.example:8443/v1/update/?secret=hunter2").unwrap();
        let logged = redacted(&url);
        assert_eq!(logged, "https://nebraska.example:8443/<redacted>");
        assert!(!logged.contains("hunter2"));
    }

    #[test]
    fn redacted_endpoint_drops_userinfo() {
        let url = Url::parse("https://user:hunter2@nebraska.example/v1/update/").unwrap();
        let logged = redacted(&url);
        assert_eq!(logged, "https://nebraska.example/<redacted>");
        assert!(!logged.contains("hunter2"));
    }

    #[test]
    fn report_failure_sends_terminal_failure() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><event status="ok"/></app></response>"#,
        );
        client
            .report_failure(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap();
        let body = client.transport.last_body.borrow().clone().unwrap();
        assert!(
            body.contains(r#"<event eventtype="3" eventresult="0""#),
            "{body}"
        );
    }

    /// A zero-backoff policy so the retry-loop tests do not actually sleep.
    fn fast_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 4,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        }
    }

    #[test]
    fn retry_returns_first_success_without_retrying() {
        let calls = Cell::new(0);
        let result: Result<u32, NebraskaError> = retry(fast_policy(), || {
            calls.set(calls.get() + 1);
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retry_retries_transient_then_succeeds() {
        let calls = Cell::new(0);
        let result: Result<u32, NebraskaError> = retry(fast_policy(), || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(NebraskaError::Transport("dns not ready".into()))
            } else {
                Ok(7)
            }
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn retry_does_not_retry_permanent_error() {
        let calls = Cell::new(0);
        let result: Result<u32, NebraskaError> = retry(fast_policy(), || {
            calls.set(calls.get() + 1);
            Err(NebraskaError::UnexpectedResponse("bad".into()))
        });
        assert!(matches!(result, Err(NebraskaError::UnexpectedResponse(_))));
        assert_eq!(calls.get(), 1, "a permanent error must not be retried");
    }

    #[test]
    fn retry_gives_up_after_max_attempts() {
        let calls = Cell::new(0);
        let result: Result<u32, NebraskaError> = retry(fast_policy(), || {
            calls.set(calls.get() + 1);
            Err(NebraskaError::Transport("still down".into()))
        });
        assert!(matches!(result, Err(NebraskaError::Transport(_))));
        assert_eq!(calls.get(), 4, "should stop at max_attempts");
    }

    #[test]
    fn try_complete_after_reboot_is_single_attempt() {
        // A transport that always fails transiently: the non-retrying variant
        // must call it exactly once and surface the retryable error.
        struct AlwaysFails {
            calls: Cell<u32>,
        }
        impl Transport for AlwaysFails {
            fn post_xml(&self, _endpoint: &Url, _body: &[u8]) -> Result<String, NebraskaError> {
                self.calls.set(self.calls.get() + 1);
                Err(NebraskaError::Transport("down".into()))
            }
        }
        let client = Client::with_transport(
            Url::parse("https://nebraska.example/v1/update/").unwrap(),
            "app-1",
            "stable",
            MachineId::new("mid-1").unwrap(),
            TEST_CLIENT_VERSION,
            AlwaysFails {
                calls: Cell::new(0),
            },
        );
        let err = client
            .try_complete_after_reboot(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap_err();
        assert!(err.is_retryable());
        assert_eq!(client.transport.calls.get(), 1);
    }
}
