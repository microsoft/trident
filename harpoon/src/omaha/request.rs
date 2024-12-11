use quick_xml::{
    events::{BytesDecl, Event},
    Writer,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use osutils::{arch::SystemArchitecture, machine_id::MachineId, osrelease::OsRelease};

use crate::error::HarpoonError;

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
                    SystemArchitecture::X86 => "x86",
                    SystemArchitecture::Amd64 => "amd64",
                    SystemArchitecture::Arm => "arm",
                    SystemArchitecture::Aarch64 => "arm64",
                    SystemArchitecture::Other => "other",
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

    #[serde(rename = "@track")]
    track: String,

    #[serde(rename = "@machineid")]
    machine_id: Uuid,

    #[serde(rename = "updatecheck", skip_serializing_if = "Option::is_none")]
    update_check: Option<UpdateCheckRequest>,

    #[serde(rename = "event", skip_serializing_if = "Vec::is_empty")]
    events: Vec<OmahaEvent>,
}

impl AppRequest {
    /// Creates a new `AppRequest` with the given `app_id`, `version`, and
    /// `track`. The `machine_id` is read from the system.
    pub(crate) fn new(
        app_id: impl Into<String>,
        version: impl Into<AppVersion>,
        track: impl Into<String>,
    ) -> Result<Self, HarpoonError> {
        Ok(Self::new_with_machine_id(
            app_id,
            version,
            track,
            MachineId::read()
                .map_err(|err| HarpoonError::MachineIdRead(err.to_string()))?
                .hashed_uuid(),
        ))
    }

    /// Creates a new `AppRequest` with the given `app_id` to be used to send
    /// update events to the server.
    pub(crate) fn new_event(
        app_id: impl Into<String>,
        track: impl Into<String>,
    ) -> Result<Self, HarpoonError> {
        Ok(Self::new_with_machine_id(
            app_id,
            AppVersion::default(),
            track,
            MachineId::read()
                .map_err(|err| HarpoonError::MachineIdRead(err.to_string()))?
                .hashed_uuid(),
        ))
    }

    pub(crate) fn new_with_machine_id(
        app_id: impl Into<String>,
        version: impl Into<AppVersion>,
        track: impl Into<String>,
        machine_id: Uuid,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            version: version.into(),
            next_version: None,
            track: track.into(),
            machine_id,
            update_check: None,
            events: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_next_version(mut self, next_version: impl Into<AppVersion>) -> Self {
        self.next_version = Some(next_version.into());
        self
    }

    pub(crate) fn with_update_check(mut self) -> Self {
        self.update_check = Some(UpdateCheckRequest);
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

#[cfg(test)]
mod tests {
    use crate::{omaha::event::OmahaEventType, EventResult};

    use super::*;

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
                SystemArchitecture::X86 => "x86",
                SystemArchitecture::Amd64 => "amd64",
                SystemArchitecture::Arm => "arm",
                SystemArchitecture::Aarch64 => "arm64",
                SystemArchitecture::Other => "other",
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
        let app = AppRequest::new("app_id", AppVersion::default(), "track").unwrap();
        let request = Request::default().with_app(app);
        assert_eq!(request.apps().len(), 1);
    }

    #[test]
    fn test_app_request_new() {
        let app = AppRequest::new("app_id", AppVersion::default(), "track").unwrap();
        assert_eq!(app.app_id(), "app_id");
        assert_eq!(app.version, AppVersion::default());
        assert_eq!(app.next_version, None);
        assert_eq!(app.track, "track");
        assert_eq!(app.machine_id, MachineId::read().unwrap().hashed_uuid());
        assert_eq!(app.update_check, None);
        assert_eq!(app.events, Vec::new());
    }

    #[test]
    fn test_app_request_new_with_machine_id() {
        let machine_id = Uuid::new_v4();
        let app =
            AppRequest::new_with_machine_id("app_id", AppVersion::default(), "track", machine_id);
        assert_eq!(app.machine_id, machine_id);
    }

    #[test]
    fn test_app_request_with_next_version() {
        let app = AppRequest::new("app_id", AppVersion::default(), "track").unwrap();
        let next_version = AppVersion::default();
        let app = app.with_next_version(next_version.clone());
        assert_eq!(app.next_version, Some(next_version));
    }

    #[test]
    fn test_app_request_with_update_check() {
        let app = AppRequest::new("app_id", AppVersion::default(), "track").unwrap();
        let app = app.with_update_check();
        assert_eq!(app.update_check, Some(UpdateCheckRequest));
    }

    #[test]
    fn test_app_request_with_event() {
        let app = AppRequest::new("app_id", AppVersion::default(), "track").unwrap();
        let event = OmahaEvent::new(OmahaEventType::Unknown, EventResult::Error);
        let app = app.with_event(event);
        assert_eq!(app.events().len(), 1);
    }

    #[test]
    fn test_app_new_event() {
        let app = AppRequest::new_event("app_id", "track").unwrap();
        assert_eq!(app.app_id(), "app_id");
        assert_eq!(app.version, AppVersion::default());
        assert_eq!(app.next_version, None);
        assert_eq!(app.track, "track");
        assert_eq!(app.machine_id, MachineId::read().unwrap().hashed_uuid());
        assert_eq!(app.update_check, None);
        assert_eq!(app.events, Vec::new());
    }
}
