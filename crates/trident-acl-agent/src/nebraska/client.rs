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
    /// `codebase` with [`name`](PackageFile::name).
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
    transport: T,
}

impl Client<ReqwestTransport> {
    /// Creates a client using the default blocking `reqwest` transport.
    ///
    /// `endpoint` should be the Nebraska update URL (typically ending in
    /// `/v1/update/`, with the trailing slash preserved).
    pub fn new(
        endpoint: Url,
        app_id: impl Into<String>,
        track: impl Into<String>,
        machine_id: MachineId,
    ) -> Self {
        Self::with_transport(endpoint, app_id, track, machine_id, ReqwestTransport::new())
    }
}

impl<T: Transport> Client<T> {
    /// Creates a client with an explicit [`Transport`], primarily for testing.
    pub fn with_transport(
        endpoint: Url,
        app_id: impl Into<String>,
        track: impl Into<String>,
        machine_id: MachineId,
        transport: T,
    ) -> Self {
        Self {
            endpoint,
            app_id: app_id.into(),
            track: track.into(),
            machine_id,
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
        self.require_app_present(&response)?;
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
    /// exponential backoff (a handful of attempts over a few tens of seconds) and
    /// returns the last error only once transient retries are exhausted or a
    /// permanent error occurs.
    ///
    /// This blocks the calling thread while retrying. A caller that has its own
    /// scheduler (e.g. an existing poll loop) and would rather re-attempt on its
    /// own cadence should use [`try_complete_after_reboot`](Client::try_complete_after_reboot)
    /// instead and drive the retry itself. If completion genuinely cannot be
    /// reported, see [`report_failure`](Client::report_failure) for the recovery
    /// path.
    ///
    /// A still-`UpdateInProgress` outcome (the completion report did not take)
    /// is also retried here, not just transport/HTTP failures: it is treated as
    /// [`NebraskaError::CompletionNotAcknowledged`] internally so the same
    /// retry loop covers it, then unwrapped back into a plain `CheckOutcome` on
    /// success.
    ///
    /// `previous_version` is the version the instance was on before the update;
    /// `current_version` is the (new) version now running.
    pub fn complete_after_reboot(
        &self,
        previous_version: &Version,
        current_version: &Version,
    ) -> Result<CheckOutcome, NebraskaError> {
        retry(RetryPolicy::default(), || {
            match self.try_complete_after_reboot(previous_version, current_version)? {
                // Nebraska hasn't reflected the completion report yet. This is
                // exactly the "the completion did not take, retry" case
                // documented on `try_complete_after_reboot`, but `retry` only
                // re-attempts on `Err`, so it must be translated into one here
                // or a still-in-progress `Ok` would be (incorrectly) treated
                // as success and returned immediately without retrying. See
                // `NebraskaError::CompletionNotAcknowledged`.
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
    /// returned error [is retryable](NebraskaError::is_retryable) (with a bounded
    /// backoff), giving up only on a permanent error. See
    /// [`complete_after_reboot`](Client::complete_after_reboot) for the rationale
    /// and [`report_failure`](Client::report_failure) for the recovery path.
    pub fn try_complete_after_reboot(
        &self,
        previous_version: &Version,
        current_version: &Version,
    ) -> Result<CheckOutcome, NebraskaError> {
        let app = self
            .app(current_version)
            .with_event(TerminalEvent::Completed.wire())
            .with_previous_version(previous_version.to_string())
            .with_ping()
            .with_update_check();
        let response = self.send(app)?;
        // The instance should now be Complete; a still-in-progress status means
        // the completion did not take and the caller should retry.
        self.interpret_check(response)
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
            .with_event(TerminalEvent::Failed.wire())
            .with_previous_version(previous_version.to_string());
        let response = self.send(app)?;
        self.require_app_present(&response)?;
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
        let request = wire::request_for(app);
        let body = request
            .to_xml()
            .map_err(|e| NebraskaError::Serialize(e.to_string()))?;
        trace!(
            "Nebraska request to '{}':\n{}",
            self.endpoint,
            String::from_utf8_lossy(&body)
        );
        let text = self.transport.post_xml(&self.endpoint, &body)?;
        trace!("Nebraska response:\n{text}");
        wire::parse_response(&text).map_err(NebraskaError::Parse)
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

    /// Validates the app is present (used by event requests, which have no
    /// update-check to interpret).
    fn require_app_present(&self, response: &wire::Response) -> Result<(), NebraskaError> {
        self.app_response(response).map(|_| ())
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
        // Join the codebase (which must end in a trailing slash) with the file
        // name to get the absolute URL; do not otherwise rewrite it.
        let url = codebase.join(&package.name).map_err(|e| {
            NebraskaError::UnexpectedResponse(format!(
                "failed to join codebase '{codebase}' with package '{}': {e}",
                package.name
            ))
        })?;

        let hash = package.hash.as_ref().map(|sha1| PackageHash {
            sha1: sha1.clone(),
            sha256: package.hash_sha256.clone(),
        });

        // Size is a string on the wire; surface it as a number when it parses,
        // and treat an unparseable size as absent rather than failing the offer.
        let size = package.size.as_ref().and_then(|s| s.parse::<u64>().ok());

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
    use std::collections::VecDeque;

    use super::*;

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
            MockTransport::new(response),
        )
    }

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
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"/></response>"#,
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
    fn complete_after_reboot_batches_event_ping_and_check() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="noupdate"/></app></response>"#,
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
    fn report_failure_sends_terminal_failure() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"/></response>"#,
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

    /// A transport that returns a different canned response on each
    /// successive call, so `complete_after_reboot`'s retry behaviour can be
    /// exercised across several attempts.
    struct SequenceTransport {
        responses: RefCell<VecDeque<String>>,
        calls: Cell<u32>,
    }

    impl SequenceTransport {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: RefCell::new(responses.into_iter().map(String::from).collect()),
                calls: Cell::new(0),
            }
        }
    }

    impl Transport for SequenceTransport {
        fn post_xml(&self, _endpoint: &Url, _body: &[u8]) -> Result<String, NebraskaError> {
            self.calls.set(self.calls.get() + 1);
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| NebraskaError::Transport("no more canned responses".into()))
        }
    }

    const STILL_IN_PROGRESS: &str = r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-updateInProgressOnInstance"><updatecheck status="error-internal"/></app></response>"#;
    const COMPLETE: &str = r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="noupdate"/></app></response>"#;

    #[test]
    fn complete_after_reboot_retries_when_still_in_progress() {
        // The first two attempts land but Nebraska hasn't reflected the
        // completion yet; the third attempt reports it as done.
        let client = Client::with_transport(
            Url::parse("https://nebraska.example/v1/update/").unwrap(),
            "app-1",
            "stable",
            MachineId::new("mid-1").unwrap(),
            SequenceTransport::new([STILL_IN_PROGRESS, STILL_IN_PROGRESS, COMPLETE]),
        );
        let outcome = client
            .complete_after_reboot(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap();
        assert_eq!(outcome, CheckOutcome::UpToDate);
        assert_eq!(
            client.transport.calls.get(),
            3,
            "a still-in-progress outcome must be retried, not returned as success"
        );
    }

    #[test]
    fn complete_after_reboot_gives_up_if_never_acknowledged() {
        // Nebraska never reflects the completion within the retry budget:
        // the call must eventually surface an error rather than silently
        // "succeeding" with a stale in-progress outcome.
        let responses = std::iter::repeat_n(STILL_IN_PROGRESS, 10);
        let client = Client::with_transport(
            Url::parse("https://nebraska.example/v1/update/").unwrap(),
            "app-1",
            "stable",
            MachineId::new("mid-1").unwrap(),
            SequenceTransport::new(responses),
        );
        let err = client
            .complete_after_reboot(&Version::new(1, 0, 0), &Version::new(2, 0, 0))
            .unwrap_err();
        assert!(matches!(err, NebraskaError::CompletionNotAcknowledged));
    }
}
