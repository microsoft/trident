use serde::Deserialize;
use url::Url;

use crate::{def_unwrap_list, error::HarpoonError};

use super::{
    app::AppVersion,
    event::EventAcknowledge,
    status::{AppStatus, UpdateCheckStatus},
    OMAHA_VERSION,
};

#[derive(Debug, Deserialize)]
pub(crate) struct Response {
    #[serde(rename = "@protocol")]
    protocol: String,

    #[serde(rename = "@server")]
    _server: String,

    #[serde(rename = "daystart")]
    _daystart: Daystart,

    #[serde(default, rename = "app")]
    apps: Vec<AppResponse>,
}

impl Response {
    pub(crate) fn validate(&self) -> Result<(), HarpoonError> {
        if self.protocol != OMAHA_VERSION {
            return Err(HarpoonError::InvalidResponse(format!(
                "Invalid Omaha version '{}', expected '{}'",
                self.protocol, OMAHA_VERSION
            )));
        }

        Ok(())
    }

    pub(crate) fn apps(&self) -> &[AppResponse] {
        &self.apps
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AppResponse {
    #[serde(rename = "@appid")]
    app_id: String,

    #[serde(rename = "@status")]
    status: AppStatus,

    #[serde(default, rename = "updatecheck")]
    update_check: Option<UpdateCheckResponse>,

    #[serde(default, rename = "event")]
    events: Vec<EventAcknowledge>,
}

impl AppResponse {
    pub(crate) fn app_id(&self) -> &str {
        &self.app_id
    }

    pub(crate) fn status(&self) -> &AppStatus {
        &self.status
    }

    pub(crate) fn update_check(&self) -> Option<&UpdateCheckResponse> {
        self.update_check.as_ref()
    }

    pub(crate) fn events(&self) -> &[EventAcknowledge] {
        &self.events
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct Daystart {
    #[serde(rename = "@elapsed_seconds")]
    pub(crate) elapsed_seconds: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateCheckResponse {
    #[serde(rename = "@status")]
    status: UpdateCheckStatus,

    #[serde(rename = "urls", deserialize_with = "unwrap_urls")]
    urls: Vec<DownloadUrl>,

    #[serde(rename = "manifest")]
    manifest: Option<Manifest>,
}

impl UpdateCheckResponse {
    pub(crate) fn status(&self) -> &UpdateCheckStatus {
        &self.status
    }

    pub(crate) fn urls(&self) -> impl Iterator<Item = &Url> {
        self.urls.iter().map(|url| &url.codebase)
    }

    pub(crate) fn version(&self) -> Option<&AppVersion> {
        self.manifest.as_ref().map(|m| &m.version)
    }

    pub(crate) fn packages(&self) -> &[Package] {
        self.manifest.as_ref().map_or(&[], |m| &m.packages)
    }
}

#[derive(Debug, Deserialize)]
struct DownloadUrl {
    #[serde(rename = "@codebase")]
    codebase: Url,
}

def_unwrap_list!(unwrap_urls, DownloadUrl, "url");

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(rename = "@version")]
    version: AppVersion,
    #[serde(default, rename = "packages", deserialize_with = "unwrap_packages")]
    packages: Vec<Package>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct Package {
    #[serde(rename = "@hash")]
    pub(crate) hash: String,

    #[serde(rename = "@hash_sha256")]
    pub(crate) hash_sha256: Option<String>,

    #[serde(rename = "@name")]
    pub(crate) name: String,

    #[serde(rename = "@size")]
    pub(crate) size: u64,

    #[serde(rename = "@required")]
    pub(crate) required: bool,
}

def_unwrap_list!(unwrap_packages, Package, "package");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_response() {
        let mut response = Response {
            protocol: OMAHA_VERSION.to_string(),
            _server: "server".to_string(),
            _daystart: Daystart { elapsed_seconds: 0 },
            apps: vec![],
        };

        response.validate().unwrap();

        response.protocol = "invalid".to_string();
        response.validate().unwrap_err();
    }

    #[test]
    fn test_parse_simple() {
        let response = indoc::indoc! {r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <response protocol="3.0"
              server="nebraska">
              <daystart elapsed_seconds="0"></daystart>
              <app appid="com.microsoft.azurelinux"
                status="ok">
              </app>
            </response>
            "#
        };

        let response: Response = quick_xml::de::from_str(response).unwrap();
        assert_eq!(response.protocol, OMAHA_VERSION);
        assert_eq!(response._server, "nebraska");
        assert_eq!(response._daystart.elapsed_seconds, 0);
        assert_eq!(response.apps.len(), 1);
        assert_eq!(response.apps[0].app_id(), "com.microsoft.azurelinux");
        assert_eq!(response.apps[0].status(), &AppStatus::Ok);
    }

    #[test]
    fn test_parse_update_check_noupdate() {
        let response = indoc::indoc! {r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <response protocol="3.0"
              server="nebraska">
              <daystart elapsed_seconds="0"></daystart>
              <app appid="com.microsoft.azurelinux"
                status="ok">
                <updatecheck status="noupdate">
                  <urls></urls>
                </updatecheck>
              </app>
            </response>
            "#
        };
        let response: Response = quick_xml::de::from_str(response).unwrap();
        let app = &response.apps[0];
        let update_check = app.update_check().unwrap();
        assert_eq!(update_check.status(), &UpdateCheckStatus::NoUpdate);
        assert_eq!(update_check.urls().count(), 0);
        assert!(update_check.manifest.is_none());
    }

    #[test]
    fn test_parse_update_check_update() {
        let response = indoc::indoc! {r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <response protocol="3.0"
              server="nebraska">
              <daystart elapsed_seconds="0"></daystart>
              <app appid="com.microsoft.azurelinux"
                status="ok">
                <updatecheck status="ok">
                  <urls>
                    <url codebase="https://example.com/"></url>
                  </urls>
                  <manifest version="2.0.2">
                    <packages>
                      <package hash="hash"
                        hash_sha256="hash_sha256"
                        name="package"
                        size="123"
                        required="true"></package>
                    </packages>
                  </manifest>
                </updatecheck>
              </app>
            </response>
            "#};
        let response: Response = quick_xml::de::from_str(response).unwrap();
        let app = &response.apps[0];
        let update_check = app.update_check().unwrap();
        assert_eq!(update_check.status, UpdateCheckStatus::Ok);
        assert_eq!(update_check.urls.len(), 1);
        assert_eq!(
            update_check.urls[0].codebase,
            Url::parse("https://example.com/").unwrap()
        );
        assert_eq!(update_check.version().unwrap(), &AppVersion::new(2, 0, 2));
        assert_eq!(update_check.packages().len(), 1);
        let package = &update_check.packages()[0];
        assert_eq!(package.hash, "hash");
        assert_eq!(package.hash_sha256.as_deref(), Some("hash_sha256"));
        assert_eq!(package.name, "package");
        assert_eq!(package.size, 123);
        assert!(package.required);
    }

    #[test]
    fn test_parse_event_response() {
        let response = indoc::indoc! {r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <response protocol="3.0"
              server="nebraska">
              <daystart elapsed_seconds="0"></daystart>
              <app appid="com.microsoft.azurelinux"
                status="ok">
                <event status="ok"></event>
              </app>
            </response>
            "#};
        let response: Response = quick_xml::de::from_str(response).unwrap();
        let app = &response.apps()[0];
        let events = app.events();
        assert_eq!(events.len(), 1);
        assert!(events[0].is_ok());
    }
}
