//! Private serde types for the Omaha request/response XML wire format.
//!
//! These are an implementation detail of the [`nebraska`](crate::nebraska)
//! module and are never exposed publicly; callers interact with the higher-level
//! [`Client`](crate::nebraska::Client) API. Keeping them private means the
//! protocol's invariants (whitelisted events, mandatory `track`, unbraced
//! machine id) can only be satisfied through the validated builders.

use quick_xml::{
    events::{BytesDecl, Event},
    Writer,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use sysdefs::arch::SystemArchitecture;

use super::{
    event::WirePair,
    status::{AppStatus, EventStatus, UpdateCheckStatus},
};

const OMAHA_PROTOCOL: &str = "3.0";
const XML_VERSION: &str = "1.0";
const XML_ENCODING: &str = "UTF-8";

/// Serializes an `ismachine`-style boolean as the string `"1"` or `"0"`, as the
/// Omaha protocol requires.
fn bool_as_num<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(if *value { "1" } else { "0" })
}

/// An outgoing Omaha `<request>`.
#[derive(Debug, Serialize)]
pub(super) struct Request {
    #[serde(rename = "@protocol")]
    protocol: &'static str,

    #[serde(rename = "@version", skip_serializing_if = "Option::is_none")]
    version: Option<String>,

    #[serde(rename = "@ismachine", serialize_with = "bool_as_num")]
    is_machine: bool,

    #[serde(rename = "@sessionid")]
    session_id: Uuid,

    #[serde(rename = "os")]
    os: Os,

    #[serde(rename = "app")]
    app: App,
}

impl Request {
    /// Builds a request carrying a single app, identifying the updater as
    /// `client_version` when the caller supplied one.
    pub(super) fn new(app: App, client_version: Option<String>) -> Self {
        Self {
            protocol: OMAHA_PROTOCOL,
            version: client_version,
            is_machine: true,
            session_id: Uuid::new_v4(),
            os: Os::current(),
            app,
        }
    }

    /// Serializes the request to UTF-8 XML bytes, including the XML declaration.
    pub(super) fn to_xml(&self) -> Result<Vec<u8>, quick_xml::SeError> {
        let mut buf = Vec::new();
        let mut writer = Writer::new(&mut buf);
        writer.write_event(Event::Decl(BytesDecl::new(
            XML_VERSION,
            Some(XML_ENCODING),
            None,
        )))?;
        writer.write_serializable("request", self)?;
        Ok(buf)
    }
}

/// The `<os>` element. Informational; Nebraska keys update decisions off the
/// `<app version>` attribute rather than this.
#[derive(Debug, Serialize)]
pub(super) struct Os {
    #[serde(rename = "@platform")]
    platform: &'static str,

    #[serde(rename = "@version")]
    version: String,

    #[serde(rename = "@arch")]
    arch: &'static str,
}

impl Os {
    fn current() -> Self {
        Self {
            platform: "linux",
            version: String::new(),
            arch: arch_str(SystemArchitecture::current()),
        }
    }
}

/// Returns the Omaha `<os arch>` value for `arch`.
///
/// Nebraska matches this against a fixed table — `amd64`/`x64` and
/// `aarch64`/`arm` — and **silently falls back to amd64** for anything it does
/// not recognise. Since the group is keyed on the architecture, an unrecognised
/// value makes every lookup miss (or, worse, match an amd64 group on the same
/// track), so the mapping is spelled out here rather than delegated to
/// `sysdefs`'s own string form: that form is `arm64`, which is *not* in
/// Nebraska's table and would send every ARM64 instance to an amd64 group.
fn arch_str(arch: SystemArchitecture) -> &'static str {
    match arch {
        SystemArchitecture::Amd64 => "amd64",
        SystemArchitecture::Aarch64 => "aarch64",
    }
}

/// The `<app>` element of a request.
#[derive(Debug, Serialize)]
pub(super) struct App {
    #[serde(rename = "@appid")]
    app_id: String,

    #[serde(rename = "@version")]
    version: String,

    #[serde(rename = "@track")]
    track: String,

    #[serde(rename = "@machineid")]
    machine_id: String,

    // Child elements are declared — and therefore serialized — in the order
    // Nebraska logically processes them: events first, then the ping, then the
    // update check. Nebraska actually processes events before the update check
    // regardless of XML order, but emitting them in this order keeps the batched
    // post-reboot request self-documenting.
    #[serde(rename = "event", skip_serializing_if = "Vec::is_empty")]
    events: Vec<EventElement>,

    #[serde(rename = "ping", skip_serializing_if = "Option::is_none")]
    ping: Option<Ping>,

    #[serde(rename = "updatecheck", skip_serializing_if = "Option::is_none")]
    update_check: Option<UpdateCheck>,
}

impl App {
    /// Creates a new `<app>` with the mandatory identity fields. `track` is a
    /// required parameter here — the type cannot be built without it — which is
    /// how the module guarantees `track` is present on every request, including
    /// event-only ones (Nebraska resolves the group from `track` before
    /// processing events, so omitting it silently drops them).
    pub(super) fn new(app_id: String, version: String, track: String, machine_id: String) -> Self {
        Self {
            app_id,
            version,
            track,
            machine_id,
            update_check: None,
            ping: None,
            events: Vec::new(),
        }
    }

    pub(super) fn with_update_check(mut self) -> Self {
        self.update_check = Some(UpdateCheck);
        self
    }

    pub(super) fn with_ping(mut self) -> Self {
        self.ping = Some(Ping { active: 1 });
        self
    }

    pub(super) fn with_event(mut self, pair: WirePair) -> Self {
        self.events.push(EventElement {
            event_type: pair.event_type,
            event_result: pair.event_result,
            previous_version: None,
        });
        self
    }

    /// Adds an event that reports the version the instance is updating *from*.
    ///
    /// `previousversion` is an attribute of `<event>`, not of `<app>`: an Omaha
    /// server parses it off the event element and ignores it anywhere else, and
    /// Nebraska discards a terminal `complete` event that arrives without it —
    /// while still answering `status="ok"` — so misplacing it loses the event
    /// silently.
    pub(super) fn with_event_from_version(mut self, pair: WirePair, previous: String) -> Self {
        self.events.push(EventElement {
            event_type: pair.event_type,
            event_result: pair.event_result,
            previous_version: Some(previous),
        });
        self
    }

    /// Sets the `<os version>` attribute (informational). Kept in sync with the
    /// app version by the client.
    pub(super) fn os_version(&self) -> &str {
        &self.version
    }
}

#[derive(Debug, Serialize)]
pub(super) struct UpdateCheck;

#[derive(Debug, Serialize)]
pub(super) struct Ping {
    #[serde(rename = "@active")]
    active: u8,
}

#[derive(Debug, Serialize)]
pub(super) struct EventElement {
    #[serde(rename = "@eventtype")]
    event_type: u16,

    #[serde(rename = "@eventresult")]
    event_result: u8,

    #[serde(rename = "@previousversion", skip_serializing_if = "Option::is_none")]
    previous_version: Option<String>,
}

/// Builds a request from an app, setting the `<os version>` from the app version
/// so the two agree.
/// Builds a request from an app, setting the `<os version>` from the app version
/// so the two agree. `client_version` identifies the updater itself, and is
/// omitted from the request entirely when the caller did not name one.
pub(super) fn request_for(app: App, client_version: Option<String>) -> Request {
    let mut request = Request::new(app, client_version);
    request.os.version = request.app.os_version().to_string();
    request
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// An incoming Omaha `<response>`.
#[derive(Debug, Deserialize)]
pub(super) struct Response {
    #[serde(default, rename = "app")]
    pub(super) apps: Vec<AppResponse>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AppResponse {
    #[serde(rename = "@appid")]
    pub(super) app_id: String,

    #[serde(rename = "@status")]
    pub(super) status: AppStatus,

    /// The per-event acknowledgements, one per event sent in the request.
    /// Empty for requests that carried no events.
    #[serde(default, rename = "event")]
    pub(super) events: Vec<EventResponse>,

    #[serde(default, rename = "updatecheck")]
    pub(super) update_check: Option<UpdateCheckResponse>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EventResponse {
    #[serde(rename = "@status")]
    pub(super) status: EventStatus,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateCheckResponse {
    #[serde(rename = "@status")]
    pub(super) status: UpdateCheckStatus,

    #[serde(default, rename = "urls")]
    pub(super) urls: Option<Urls>,

    #[serde(rename = "manifest")]
    pub(super) manifest: Option<Manifest>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Urls {
    #[serde(default, rename = "url")]
    pub(super) urls: Vec<UrlElement>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UrlElement {
    #[serde(rename = "@codebase")]
    pub(super) codebase: Url,
}

#[derive(Debug, Deserialize)]
pub(super) struct Manifest {
    #[serde(rename = "@version")]
    pub(super) version: String,

    #[serde(default, rename = "packages")]
    pub(super) packages: Option<Packages>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Packages {
    #[serde(default, rename = "package")]
    pub(super) packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Package {
    #[serde(rename = "@name")]
    pub(super) name: String,

    /// The package hash as sent by Nebraska: base64-encoded SHA-1 of the package
    /// *file*. Optional because Nebraska omits the attribute when a package has
    /// no hash.
    #[serde(default, rename = "@hash")]
    pub(super) hash: Option<String>,

    /// The optional SHA-256 package hash, base64-encoded, when present.
    #[serde(default, rename = "@hash_sha256")]
    pub(super) hash_sha256: Option<String>,

    /// The package size in bytes, as a string in the wire format.
    #[serde(default, rename = "@size")]
    pub(super) size: Option<String>,

    /// Whether the manifest marks this file as required. Defaults to `false`
    /// when the attribute is absent.
    #[serde(default, rename = "@required")]
    pub(super) required: bool,
}

/// Parses a Nebraska response body.
pub(super) fn parse_response(body: &str) -> Result<Response, String> {
    let deserializer = &mut quick_xml::de::Deserializer::from_str(body);
    serde_path_to_error::deserialize(deserializer).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT_VERSION: &str = "test-updater-1.2.3";

    fn xml_of(app: App) -> String {
        xml_of_versioned(app, Some(CLIENT_VERSION.to_string()))
    }

    fn xml_of_versioned(app: App, client_version: Option<String>) -> String {
        String::from_utf8(request_for(app, client_version).to_xml().unwrap()).unwrap()
    }

    fn bare_app() -> App {
        App::new(
            "app-1".into(),
            "1.0.0".into(),
            "stable".into(),
            "mid-1".into(),
        )
    }

    #[test]
    fn request_omits_client_version_when_unset() {
        // The attribute is optional in the protocol; a request without it
        // decodes cleanly server-side, so send nothing rather than an empty
        // string.
        let xml = xml_of_versioned(bare_app().with_update_check(), None);
        assert!(!xml.contains("<request version="), "{xml}");
        assert!(!xml.contains(r#"version="""#), "{xml}");
    }

    #[test]
    fn request_reports_the_caller_supplied_client_version() {
        // `<request version>` identifies the updater, which is the caller's to
        // name: this module is a generic Omaha client and has no version of its
        // own to report.
        let app = App::new(
            "app-1".into(),
            "1.0.0".into(),
            "stable".into(),
            "mid-1".into(),
        )
        .with_update_check();
        let xml = xml_of(app);

        assert!(
            xml.contains(&format!(r#"version="{CLIENT_VERSION}""#)),
            "{xml}"
        );
    }

    #[test]
    fn update_check_request_shape() {
        let app = App::new(
            "app-1".into(),
            "1.0.0".into(),
            "stable".into(),
            "mid-1".into(),
        )
        .with_update_check();
        let xml = xml_of(app);

        assert!(xml.contains(r#"protocol="3.0""#), "{xml}");
        assert!(xml.contains(r#"ismachine="1""#), "{xml}");
        assert!(xml.contains(r#"appid="app-1""#), "{xml}");
        assert!(xml.contains(r#"version="1.0.0""#), "{xml}");
        assert!(xml.contains(r#"track="stable""#), "{xml}");
        assert!(xml.contains(r#"machineid="mid-1""#), "{xml}");
        assert!(xml.contains("<updatecheck"), "{xml}");
        // os version mirrors the app version
        assert!(
            xml.contains(r#"<os platform="linux" version="1.0.0""#),
            "{xml}"
        );
        // no stray event / ping / previousversion
        assert!(!xml.contains("<event"), "{xml}");
        assert!(!xml.contains("<ping"), "{xml}");
        assert!(!xml.contains("previousversion"), "{xml}");
    }

    #[test]
    fn arch_strings_are_the_ones_nebraska_parses() {
        // Nebraska matches these against a fixed table and silently falls back
        // to amd64 for anything else, so a wrong value sends instances to the
        // wrong group instead of failing loudly. Its accepted spellings are
        // "amd64"/"x64" and "aarch64"/"arm" — notably *not* "arm64".
        assert_eq!(arch_str(SystemArchitecture::Amd64), "amd64");
        assert_eq!(arch_str(SystemArchitecture::Aarch64), "aarch64");
    }

    #[test]
    fn progress_event_request_shape() {
        let app = App::new(
            "app-1".into(),
            "1.0.0".into(),
            "stable".into(),
            "mid-1".into(),
        )
        .with_event(WirePair {
            event_type: 13,
            event_result: 1,
        });
        let xml = xml_of(app);

        assert!(
            xml.contains(r#"<event eventtype="13" eventresult="1""#),
            "{xml}"
        );
        assert!(xml.contains(r#"track="stable""#), "{xml}");
        assert!(!xml.contains("<updatecheck"), "{xml}");
        assert!(!xml.contains("<ping"), "{xml}");
    }

    #[test]
    fn batched_completion_request_shape() {
        // The mandatory post-reboot request: terminal 3/2 event + ping +
        // updatecheck, all in one request, with previousversion set.
        let app = App::new(
            "app-1".into(),
            "2.0.0".into(),
            "stable".into(),
            "mid-1".into(),
        )
        .with_event_from_version(
            WirePair {
                event_type: 3,
                event_result: 2,
            },
            "1.0.0".into(),
        )
        .with_ping()
        .with_update_check();
        let xml = xml_of(app);

        assert!(
            xml.contains(r#"<event eventtype="3" eventresult="2""#),
            "{xml}"
        );
        // `previousversion` must sit on the <event>, not the <app>: an Omaha
        // server reads it off the event element and ignores it elsewhere, and
        // Nebraska drops a terminal event that arrives without it.
        assert!(
            xml.contains(r#"<event eventtype="3" eventresult="2" previousversion="1.0.0""#),
            "{xml}"
        );
        assert!(xml.contains(r#"active="1""#), "{xml}");
        assert!(xml.contains("<ping"), "{xml}");
        assert!(xml.contains("<updatecheck"), "{xml}");

        // The batching is what closes the wedge window: the elements must appear
        // together in one body. Assert the logical order event → ping →
        // updatecheck so a refactor cannot silently split or reorder them.
        let event_at = xml.find("<event").unwrap();
        let ping_at = xml.find("<ping").unwrap();
        let check_at = xml.find("<updatecheck").unwrap();
        assert!(event_at < ping_at, "event should precede ping: {xml}");
        assert!(ping_at < check_at, "ping should precede updatecheck: {xml}");
    }

    #[test]
    fn parse_update_offer() {
        // A representative positive update-check response, including the empty
        // <actions></actions> element that must not break parsing.
        let body = r#"
            <response protocol="3.0" server="nebraska">
              <daystart elapsed_seconds="0"/>
              <app appid="example-app" status="ok">
                <updatecheck status="ok">
                  <urls><url codebase="https://updates.example.com/"/></urls>
                  <manifest version="2.0.0">
                    <packages>
                      <package name="os-image-2.0.0.cosi" hash="AAAAAAAAAAAAAAAAAAAAAAAAAAA=" size="368420864" required="true"/>
                    </packages>
                    <actions></actions>
                  </manifest>
                </updatecheck>
              </app>
            </response>"#;
        let resp = parse_response(body).unwrap();
        assert_eq!(resp.apps.len(), 1);
        let app = &resp.apps[0];
        assert_eq!(app.app_id, "example-app");
        assert!(app.status.is_ok());
        let uc = app.update_check.as_ref().unwrap();
        assert!(uc.status.is_update_available());
        assert_eq!(uc.manifest.as_ref().unwrap().version, "2.0.0");
        assert_eq!(
            uc.urls.as_ref().unwrap().urls[0].codebase.as_str(),
            "https://updates.example.com/"
        );
        let pkg = &uc
            .manifest
            .as_ref()
            .unwrap()
            .packages
            .as_ref()
            .unwrap()
            .packages[0];
        assert_eq!(pkg.name, "os-image-2.0.0.cosi");
        assert_eq!(pkg.hash.as_deref(), Some("AAAAAAAAAAAAAAAAAAAAAAAAAAA="));
        assert_eq!(pkg.size.as_deref(), Some("368420864"));
    }

    #[test]
    fn parse_noupdate() {
        let body = r#"
            <response protocol="3.0" server="nebraska">
              <daystart elapsed_seconds="0"/>
              <app appid="app-1" status="ok">
                <updatecheck status="noupdate"/>
              </app>
            </response>"#;
        let resp = parse_response(body).unwrap();
        let uc = resp.apps[0].update_check.as_ref().unwrap();
        assert!(uc.status.is_no_update());
    }

    #[test]
    fn parse_update_in_progress() {
        // The shape returned on every poll between the first progress event and
        // the terminal one. Must parse cleanly (app status Other-mapped, check
        // status error-internal), never panic.
        let body = r#"
            <response protocol="3.0" server="nebraska">
              <daystart elapsed_seconds="0"/>
              <app appid="app-1" status="error-updateInProgressOnInstance">
                <updatecheck status="error-internal"/>
              </app>
            </response>"#;
        let resp = parse_response(body).unwrap();
        assert!(resp.apps[0].status.is_update_in_progress());
        assert_eq!(
            resp.apps[0].update_check.as_ref().unwrap().status,
            UpdateCheckStatus::ErrorInternal
        );
    }

    #[test]
    fn parse_unknown_status_does_not_fail() {
        let body = r#"
            <response protocol="3.0" server="nebraska">
              <daystart elapsed_seconds="0"/>
              <app appid="app-1" status="error-madeUpStatus">
                <updatecheck status="error-alsoMadeUp"/>
              </app>
            </response>"#;
        let resp = parse_response(body).unwrap();
        assert_eq!(
            resp.apps[0].status,
            AppStatus::Other("error-madeUpStatus".to_string())
        );
    }
}
