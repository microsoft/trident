//! The high-level [`Client`] for talking to a Nebraska server.

use log::{debug, trace};
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
/// the terminal event; it is expected server behaviour (protocol spec §4 and
/// §7 trap 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// No update is available; the instance is up to date.
    UpToDate,

    /// An update is available.
    UpdateAvailable(UpdateOffer),

    /// Nebraska reports an update is already in progress for this instance.
    /// Expected while an update is mid-flight; the caller should keep polling
    /// (or, post-reboot, report completion) rather than treat it as an error.
    UpdateInProgress,
}

/// An offered update: the version and the fully-resolved package URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOffer {
    /// The version being offered.
    pub version: Version,

    /// The absolute URL of the update package, resolved by joining the
    /// response's `codebase` with the package `name`.
    pub package_url: Url,
}

/// A client for a single Nebraska app on a single track.
///
/// The client bundles the immutable request identity — endpoint, app id,
/// `track`, and [`MachineId`]. Because these are required to construct the
/// client and every request flows through it, two protocol invariants hold
/// structurally: `track` is present on every request including event-only ones
/// (protocol spec §7 trap 4), and the machine id is always a validated, unbraced
/// value (§7 trap 2).
///
/// # Event ordering and the all-or-nothing rule
///
/// Emitting a [progress event](Client::report_progress) is a **commitment** to
/// eventually emit a terminal event: leaving an instance in a progress state
/// wedges it permanently, with no server-side self-heal (protocol spec §3). The
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
    /// non-semver version fails instance registration server-side (protocol spec
    /// §8).
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
    /// has caused Nebraska to grant the update (Nebraska rejects events from an
    /// instance it has never seen; protocol spec §7 trap 5). Emitting a progress
    /// event commits the caller to eventually reporting a terminal event — see
    /// the [type docs](Client).
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

    /// Reports successful completion after the reboot, in the single batched
    /// request Nebraska expects: a terminal `complete` event plus a `<ping/>`
    /// plus an `<updatecheck/>`.
    ///
    /// Nebraska processes the event before the update check within one request,
    /// so this both moves the instance to Complete and returns a clean
    /// `noupdate` in one round trip — closing the window in which a bare
    /// post-reboot poll would hit `error-updateInProgressOnInstance` (protocol
    /// spec §4). This is the terminal event that discharges the commitment made
    /// by [`report_progress`](Client::report_progress), and it must be retried
    /// until it lands: losing it wedges the instance permanently.
    ///
    /// `previous_version` is the version the instance was on before the update;
    /// `current_version` is the (new) version now running.
    pub fn complete_after_reboot(
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
    /// can grant again (protocol spec §6). This is the "reset and retry" path
    /// for a wedged or failed update.
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
        Ok(CheckOutcome::UpdateAvailable(offer))
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
        let package = match packages {
            [package] => package,
            [] => {
                return Err(NebraskaError::UnexpectedResponse(
                    "update available but no package listed".to_string(),
                ))
            }
            many => {
                return Err(NebraskaError::UnexpectedResponse(format!(
                    "expected exactly one package, found {}",
                    many.len()
                )))
            }
        };

        // Join the codebase (which must end in a trailing slash) with the
        // package name to get the absolute package URL; do not otherwise rewrite
        // it.
        let package_url = codebase.codebase.join(&package.name).map_err(|e| {
            NebraskaError::UnexpectedResponse(format!(
                "failed to join codebase '{}' with package '{}': {e}",
                codebase.codebase, package.name
            ))
        })?;

        Ok(UpdateOffer {
            version,
            package_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

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
              <urls><url codebase="http://192.168.122.1:8080/"/></urls>
              <manifest version="3.0.20260803">
                <packages><package name="acl-3.0.20260803.cosi" size="1" required="true"/></packages>
              </manifest>
            </updatecheck>
          </app>
        </response>"#;

    #[test]
    fn check_returns_offer_with_joined_url() {
        let client = client_with(OFFER);
        let outcome = client
            .check_for_update(&Version::new(3, 0, 20260731))
            .unwrap();
        match outcome {
            CheckOutcome::UpdateAvailable(offer) => {
                assert_eq!(offer.version, Version::new(3, 0, 20260803));
                assert_eq!(
                    offer.package_url.as_str(),
                    "http://192.168.122.1:8080/acl-3.0.20260803.cosi"
                );
            }
            other => panic!("expected an update offer, got {other:?}"),
        }
    }

    #[test]
    fn check_reports_no_update() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"><updatecheck status="noupdate"/></app></response>"#,
        );
        assert_eq!(
            client
                .check_for_update(&Version::new(3, 0, 20260803))
                .unwrap(),
            CheckOutcome::UpToDate
        );
    }

    #[test]
    fn check_maps_update_in_progress() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="error-updateInProgressOnInstance"><updatecheck status="error-internal"/></app></response>"#,
        );
        assert_eq!(
            client
                .check_for_update(&Version::new(3, 0, 20260803))
                .unwrap(),
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
            .report_progress(
                &Version::new(3, 0, 20260731),
                ProgressEvent::DownloadStarted,
            )
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
            .complete_after_reboot(&Version::new(3, 0, 20260731), &Version::new(3, 0, 20260803))
            .unwrap();
        assert_eq!(outcome, CheckOutcome::UpToDate);
        let body = client.transport.last_body.borrow().clone().unwrap();
        assert!(
            body.contains(r#"<event eventtype="3" eventresult="2""#),
            "{body}"
        );
        assert!(body.contains(r#"previousversion="3.0.20260731""#), "{body}");
        assert!(body.contains("<ping"), "{body}");
        assert!(body.contains("<updatecheck"), "{body}");
    }

    #[test]
    fn report_failure_sends_terminal_failure() {
        let client = client_with(
            r#"<response protocol="3.0" server="n"><app appid="app-1" status="ok"/></response>"#,
        );
        client
            .report_failure(&Version::new(3, 0, 20260731), &Version::new(3, 0, 20260803))
            .unwrap();
        let body = client.transport.last_body.borrow().clone().unwrap();
        assert!(
            body.contains(r#"<event eventtype="3" eventresult="0""#),
            "{body}"
        );
    }
}
