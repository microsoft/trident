use quick_xml::{
    events::{BytesDecl, Event},
    Writer,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use osutils::osrelease::OsRelease;
use sysdefs::arch::SystemArchitecture;

use crate::{error::HarpoonError, IdSource};

use super::{
    app::AppVersion, event::OmahaEvent, OMAHA_VERSION, XML_HEADER_ENCODING, XML_HEADER_VERSION,
};

#[derive(Debug, Serialize)]
pub(crate) struct Request {
    #[serde(rename = "@protocol")]
    protocol: &'static str,

    #[serde(rename = "@version")]
    version: &'static str,

    #[serde(rename = "@ismachine", serialize_with = "bool2num")]
    is_machine: bool,

    #[serde(rename = "@sessionid")]
    session_id: Uuid,

    #[serde(rename = "hw")]
    hw: HwData,

    #[serde(rename = "os")]
    os: OsData,

    #[serde(rename = "app")]
    apps: Vec<AppRequest>,
}

fn bool2num<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(if *value { "1" } else { "0" })
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HwData {}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OsData {
    #[serde(rename = "@arch")]
    architecture: &'static str,

    #[serde(rename = "@version", skip_serializing_if = "Option::is_none")]
    version: Option<String>,

    #[serde(rename = "@platform")]
    platform: &'static str,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            protocol: OMAHA_VERSION,
            version: env!("CARGO_PKG_VERSION"),
            is_machine: true,
            session_id: Uuid::new_v4(),
            hw: HwData {},
            os: OsData {
                platform: "linux",
                version: OsRelease::read().unwrap_or_default().version,
                architecture: match SystemArchitecture::current() {
                    SystemArchitecture::Amd64 => "amd64",
                    SystemArchitecture::Aarch64 => "arm64",
                },
            },
            apps: Vec::new(),
        }
    }
}

impl Request {
    #[allow(dead_code)]
    pub(crate) fn new_with_session_id(session_id: Uuid) -> Self {
        Self {
            session_id,
            ..Default::default()
        }
    }

    pub(crate) fn to_xml(&self) -> Result<Vec<u8>, quick_xml::SeError> {
        let mut data = Vec::new();
        let mut writer = Writer::new(&mut data);
        writer.write_event(Event::Decl(BytesDecl::new(
            XML_HEADER_VERSION,
            Some(XML_HEADER_ENCODING),
            None,
        )))?;
        writer.write_serializable("request", self)?;
        Ok(data)
    }

    pub(crate) fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub(crate) fn with_app(mut self, app: AppRequest) -> Self {
        self.apps.push(app);
        self
    }

    pub(crate) fn apps(&self) -> &[AppRequest] {
        &self.apps
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct AppRequest {
    #[serde(rename = "@appid")]
    app_id: String,

    #[serde(rename = "@version")]
    version: AppVersion,

    #[serde(rename = "@nextversion", skip_serializing_if = "Option::is_none")]
    next_version: Option<AppVersion>,

    #[serde(rename = "@previousversion", skip_serializing_if = "Option::is_none")]
    previous_version: Option<AppVersion>,

    #[serde(rename = "@track")]
    track: String,

    #[serde(rename = "@machineid")]
    machine_id: String,

    #[serde(rename = "updatecheck", skip_serializing_if = "Option::is_none")]
    update_check: Option<UpdateCheckRequest>,

    #[serde(rename = "ping", skip_serializing_if = "Option::is_none")]
    ping: Option<PingRequest>,

    #[serde(rename = "event", skip_serializing_if = "Vec::is_empty")]
    events: Vec<OmahaEvent>,
}

impl AppRequest {
    /// Creates a new `AppRequest` with the given `app_id` to be used to send
    /// update events to the server, and the given `machine_id_source` to
    /// determine the machine ID.
    pub(crate) fn new_event(
        app_id: impl Into<String>,
        track: impl Into<String>,
        machine_id_source: IdSource,
    ) -> Result<Self, HarpoonError> {
        Self::new(app_id, AppVersion::default(), track, machine_id_source)
    }

    pub(crate) fn new(
        app_id: impl Into<String>,
        version: impl Into<AppVersion>,
        track: impl Into<String>,
        machine_id_source: IdSource,
    ) -> Result<Self, HarpoonError> {
        Ok(Self::new_with_machine_id(
            app_id,
            version,
            track,
            machine_id_source.produce_id()?,
        ))
    }

    pub(crate) fn new_with_machine_id(
        app_id: impl Into<String>,
        version: impl Into<AppVersion>,
        track: impl Into<String>,
        machine_id: String,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            version: version.into(),
            next_version: None,
            previous_version: None,
            track: track.into(),
            machine_id,
            update_check: None,
            ping: None,
            events: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_next_version(mut self, next_version: impl Into<AppVersion>) -> Self {
        self.next_version = Some(next_version.into());
        self
    }

    /// Sets the `previousversion` attribute, stored by Nebraska for readable
    /// instance history (e.g. on the post-reboot update-complete event).
    pub(crate) fn with_previous_version(mut self, previous_version: impl Into<AppVersion>) -> Self {
        self.previous_version = Some(previous_version.into());
        self
    }

    pub(crate) fn with_update_check(mut self) -> Self {
        self.update_check = Some(UpdateCheckRequest);
        self
    }

    /// Adds an active `<ping/>` element, used in the batched post-reboot request.
    pub(crate) fn with_ping(mut self) -> Self {
        self.ping = Some(PingRequest { active: 1 });
        self
    }

    pub(crate) fn with_event(mut self, event: OmahaEvent) -> Self {
        self.events.push(event);
        self
    }

    pub(crate) fn events(&self) -> &[OmahaEvent] {
        &self.events
    }

    pub(crate) fn app_id(&self) -> &str {
        &self.app_id
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct UpdateCheckRequest;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct PingRequest {
    #[serde(rename = "@active")]
    active: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    use osutils::machine_id::MachineId;

    use crate::{omaha::event::OmahaEventType, EventResult};

    #[test]
    fn test_bool2num() {
        let mut serializer = serde_json::Serializer::new(Vec::new());
        bool2num(&true, &mut serializer).unwrap();
        assert_eq!(serializer.into_inner(), "\"1\"".as_bytes());

        let mut serializer = serde_json::Serializer::new(Vec::new());
        bool2num(&false, &mut serializer).unwrap();
        assert_eq!(serializer.into_inner(), "\"0\"".as_bytes());
    }

    #[test]
    fn test_batched_complete_request_xml() {
        // The post-reboot request must carry, for one app: an update-complete
        // event (3/2), a previousversion attribute, a ping, and an update-check
        // — all in a single request, with attributes before child elements.
        let app = AppRequest::new_with_machine_id(
            "app-1",
            AppVersion::new(3, 0, 20260803),
            "stable",
            "mid-123".to_string(),
        )
        .with_event(OmahaEvent::new(
            OmahaEventType::UpdateComplete,
            EventResult::SuccessReboot,
        ))
        .with_previous_version(AppVersion::new(3, 0, 20260731))
        .with_ping()
        .with_update_check();

        let xml = String::from_utf8(Request::default().with_app(app).to_xml().unwrap()).unwrap();

        assert!(xml.contains(r#"version="3.0.20260803""#), "xml: {xml}");
        assert!(xml.contains(r#"previousversion="3.0.20260731""#), "xml: {xml}");
        assert!(xml.contains(r#"track="stable""#), "xml: {xml}");
        assert!(xml.contains(r#"machineid="mid-123""#), "xml: {xml}");
        assert!(xml.contains(r#"<event eventtype="3" eventresult="2""#), "xml: {xml}");
        assert!(xml.contains("<ping"), "xml: {xml}");
        assert!(xml.contains("active=\"1\""), "xml: {xml}");
        assert!(xml.contains("<updatecheck"), "xml: {xml}");
    }

    #[test]
    fn test_single_event_request_xml() {
        // A pre-reboot event request: one event, no updatecheck, no ping.
        let app = AppRequest::new_with_machine_id(
            "app-1",
            AppVersion::new(3, 0, 20260731),
            "stable",
            "mid-123".to_string(),
        )
        .with_event(OmahaEvent::new(
            OmahaEventType::UpdateDownloadStarted,
            EventResult::Success,
        ));

        let xml = String::from_utf8(Request::default().with_app(app).to_xml().unwrap()).unwrap();

        assert!(xml.contains(r#"<event eventtype="13" eventresult="1""#), "xml: {xml}");
        assert!(!xml.contains("<updatecheck"), "xml: {xml}");
        assert!(!xml.contains("<ping"), "xml: {xml}");
        assert!(!xml.contains("previousversion"), "xml: {xml}");
    }

    #[test]
    fn test_request_default() {
        let request = Request::default();
        assert_eq!(request.protocol, OMAHA_VERSION);
        assert_eq!(request.version, env!("CARGO_PKG_VERSION"));
        assert!(request.is_machine);
        assert_eq!(request.hw, HwData {});
        assert_eq!(request.os.platform, "linux");
        assert_eq!(
            request.os.version,
            OsRelease::read().unwrap_or_default().version
        );
        assert_eq!(
            request.os.architecture,
            match SystemArchitecture::current() {
                SystemArchitecture::Amd64 => "amd64",
                SystemArchitecture::Aarch64 => "arm64",
            }
        );
        assert_eq!(request.apps(), &[]);
    }

    #[test]
    fn test_request_new_with_session_id() {
        let session_id = Uuid::new_v4();
        let request = Request::new_with_session_id(session_id);
        assert_eq!(request.session_id(), session_id);
    }

    #[test]
    fn test_request_with_app() {
        let app = AppRequest::new(
            "app_id",
            AppVersion::default(),
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();
        let request = Request::default().with_app(app);
        assert_eq!(request.apps().len(), 1);
    }

    #[test]
    fn test_app_request_new() {
        let app = AppRequest::new(
            "app_id",
            AppVersion::default(),
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();
        assert_eq!(app.app_id(), "app_id");
        assert_eq!(app.version, AppVersion::default());
        assert_eq!(app.next_version, None);
        assert_eq!(app.track, "track");
        assert_eq!(
            app.machine_id,
            MachineId::read().unwrap().hashed_uuid().to_string()
        );
        assert_eq!(app.update_check, None);
        assert_eq!(app.events, Vec::new());
    }

    #[test]
    fn test_app_request_new_with_machine_id() {
        let machine_id = Uuid::new_v4().to_string();
        let app = AppRequest::new_with_machine_id(
            "app_id",
            AppVersion::default(),
            "track",
            machine_id.clone(),
        );
        assert_eq!(app.machine_id, machine_id);
    }

    #[test]
    fn test_app_request_with_next_version() {
        let app = AppRequest::new(
            "app_id",
            AppVersion::default(),
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();
        let next_version = AppVersion::default();
        let app = app.with_next_version(next_version.clone());
        assert_eq!(app.next_version, Some(next_version));
    }

    #[test]
    fn test_app_request_with_update_check() {
        let app = AppRequest::new(
            "app_id",
            AppVersion::default(),
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();
        let app = app.with_update_check();
        assert_eq!(app.update_check, Some(UpdateCheckRequest));
    }

    #[test]
    fn test_app_request_with_event() {
        let app = AppRequest::new(
            "app_id",
            AppVersion::default(),
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();
        let event = OmahaEvent::new(OmahaEventType::Unknown, EventResult::Error);
        let app = app.with_event(event);
        assert_eq!(app.events().len(), 1);
    }

    #[test]
    fn test_app_new_event() {
        let app = AppRequest::new_event("app_id", "track", IdSource::MachineIdHashed).unwrap();
        assert_eq!(app.app_id(), "app_id");
        assert_eq!(app.version, AppVersion::default());
        assert_eq!(app.next_version, None);
        assert_eq!(app.track, "track");
        assert_eq!(
            app.machine_id,
            MachineId::read().unwrap().hashed_uuid().to_string()
        );
        assert_eq!(app.update_check, None);
        assert_eq!(app.events, Vec::new());
    }
}
