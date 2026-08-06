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

use super::{
    event::WirePair,
    status::{AppStatus, UpdateCheckStatus},
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

    #[serde(rename = "@version")]
    version: &'static str,

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
    /// Builds a request carrying a single app.
    pub(super) fn new(app: App) -> Self {
        Self {
            protocol: OMAHA_PROTOCOL,
            version: env!("CARGO_PKG_VERSION"),
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
            arch: arch_str(),
        }
    }
}

/// Returns the Omaha architecture string for the current build target.
fn arch_str() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
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

    #[serde(rename = "@previousversion", skip_serializing_if = "Option::is_none")]
    previous_version: Option<String>,

    #[serde(rename = "updatecheck", skip_serializing_if = "Option::is_none")]
    update_check: Option<UpdateCheck>,

    #[serde(rename = "ping", skip_serializing_if = "Option::is_none")]
    ping: Option<Ping>,

    #[serde(rename = "event", skip_serializing_if = "Vec::is_empty")]
    events: Vec<EventElement>,
}

impl App {
    /// Creates a new `<app>` with the mandatory identity fields. `track` is a
    /// required parameter here — the type cannot be built without it — which is
    /// how the module guarantees `track` is present on every request, including
    /// event-only ones (protocol spec §7 trap 4).
    pub(super) fn new(app_id: String, version: String, track: String, machine_id: String) -> Self {
        Self {
            app_id,
            version,
            track,
            machine_id,
            previous_version: None,
            update_check: None,
            ping: None,
            events: Vec::new(),
        }
    }

    pub(super) fn with_previous_version(mut self, previous: String) -> Self {
        self.previous_version = Some(previous);
        self
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
}

/// Builds a request from an app, setting the `<os version>` from the app version
/// so the two agree.
pub(super) fn request_for(app: App) -> Request {
    let mut request = Request::new(app);
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

    #[serde(default, rename = "updatecheck")]
    pub(super) update_check: Option<UpdateCheckResponse>,
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
}

/// Parses a Nebraska response body.
pub(super) fn parse_response(body: &str) -> Result<Response, String> {
    let deserializer = &mut quick_xml::de::Deserializer::from_str(body);
    serde_path_to_error::deserialize(deserializer).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml_of(app: App) -> String {
        String::from_utf8(request_for(app).to_xml().unwrap()).unwrap()
    }

    #[test]
    fn update_check_request_shape() {
        let app = App::new(
            "app-1".into(),
            "3.0.20260731".into(),
            "stable".into(),
            "mid-1".into(),
        )
        .with_update_check();
        let xml = xml_of(app);

        assert!(xml.contains(r#"protocol="3.0""#), "{xml}");
        assert!(xml.contains(r#"ismachine="1""#), "{xml}");
        assert!(xml.contains(r#"appid="app-1""#), "{xml}");
        assert!(xml.contains(r#"version="3.0.20260731""#), "{xml}");
        assert!(xml.contains(r#"track="stable""#), "{xml}");
        assert!(xml.contains(r#"machineid="mid-1""#), "{xml}");
        assert!(xml.contains("<updatecheck"), "{xml}");
        // os version mirrors the app version
        assert!(
            xml.contains(r#"<os platform="linux" version="3.0.20260731""#),
            "{xml}"
        );
        // no stray event / ping / previousversion
        assert!(!xml.contains("<event"), "{xml}");
        assert!(!xml.contains("<ping"), "{xml}");
        assert!(!xml.contains("previousversion"), "{xml}");
    }

    #[test]
    fn progress_event_request_shape() {
        let app = App::new(
            "app-1".into(),
            "3.0.20260731".into(),
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
            "3.0.20260803".into(),
            "stable".into(),
            "mid-1".into(),
        )
        .with_event(WirePair {
            event_type: 3,
            event_result: 2,
        })
        .with_previous_version("3.0.20260731".into())
        .with_ping()
        .with_update_check();
        let xml = xml_of(app);

        assert!(
            xml.contains(r#"<event eventtype="3" eventresult="2""#),
            "{xml}"
        );
        assert!(xml.contains(r#"previousversion="3.0.20260731""#), "{xml}");
        assert!(xml.contains(r#"active="1""#), "{xml}");
        assert!(xml.contains("<ping"), "{xml}");
        assert!(xml.contains("<updatecheck"), "{xml}");
    }

    #[test]
    fn parse_update_offer() {
        // Captured shape from the demo (protocol spec §4), including the empty
        // <actions></actions> that must not break parsing.
        let body = r#"
            <response protocol="3.0" server="nebraska">
              <daystart elapsed_seconds="0"/>
              <app appid="6d10cf97-443f-4542-8479-b9fdb44c9588" status="ok">
                <updatecheck status="ok">
                  <urls><url codebase="http://192.168.122.1:8080/"/></urls>
                  <manifest version="3.0.20260803">
                    <packages>
                      <package name="acl-3.0.20260803.cosi" hash="5RKW+UWMc6TGENo7KO+ZCvk5EM4=" size="368420864" required="true"/>
                    </packages>
                    <actions></actions>
                  </manifest>
                </updatecheck>
              </app>
            </response>"#;
        let resp = parse_response(body).unwrap();
        assert_eq!(resp.apps.len(), 1);
        let app = &resp.apps[0];
        assert_eq!(app.app_id, "6d10cf97-443f-4542-8479-b9fdb44c9588");
        assert!(app.status.is_ok());
        let uc = app.update_check.as_ref().unwrap();
        assert!(uc.status.is_update_available());
        assert_eq!(uc.manifest.as_ref().unwrap().version, "3.0.20260803");
        assert_eq!(
            uc.urls.as_ref().unwrap().urls[0].codebase.as_str(),
            "http://192.168.122.1:8080/"
        );
        assert_eq!(
            uc.manifest
                .as_ref()
                .unwrap()
                .packages
                .as_ref()
                .unwrap()
                .packages[0]
                .name,
            "acl-3.0.20260803.cosi"
        );
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
