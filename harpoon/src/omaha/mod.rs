//! Super basic implementation of the Omaha protocol as defined in
//! https://github.com/google/omaha/blob/main/doc/ServerProtocol.md.

use log::{debug, trace};
use url::Url;

pub(crate) mod app;
pub(crate) mod event;
pub(crate) mod request;
pub(crate) mod response;
pub(crate) mod status;
mod xml;

use request::Request;
use response::Response;

use crate::error::HarpoonError;

const OMAHA_VERSION: &str = "3.0";
const XML_HEADER_VERSION: &str = "1.0";
const XML_HEADER_ENCODING: &str = "UTF-8";

/// Sends a generic request to the Omaha server at the given URL and returns the
/// resulting response.
pub(crate) fn send(url: &Url, req: &Request) -> Result<Response, HarpoonError> {
    let body = req
        .to_xml()
        .map_err(|e| HarpoonError::Internal(format!("Failed to serialize request XML: {e}")))?;

    debug!("Sending Omaha request to '{url}'",);
    trace!("Omaha request body:\n{}", String::from_utf8_lossy(&body));
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(url.as_str())
        .header("Content-Type", "application/xml")
        .body(body)
        .send()
        .map_err(|e| HarpoonError::SendRequest(e.to_string()))?
        .error_for_status()
        .map_err(|e| HarpoonError::HttpError(e.to_string()))?;

    let text = response
        .text()
        .map_err(|e| HarpoonError::HttpError(e.to_string()))?;

    trace!("Omaha response body:\n{}", text);

    let xmld = &mut quick_xml::de::Deserializer::from_str(&text);
    let response: Response = serde_path_to_error::deserialize(xmld)
        .map_err(|e| HarpoonError::ParseResponse(e.to_string()))?;

    trace!("Parsed response body:\n{:#?}", response);

    response.validate()?;

    Ok(response)
}

/// Sends an event request to the Omaha server at the given URL and returns the
/// resulting response.
pub(crate) fn send_event(url: &Url, req: &Request) -> Result<Response, HarpoonError> {
    // Send the response and get the response
    let response: Response = send(url, req)?;

    // Validate that all the events of the request were acknowledged.
    for (app, events) in req.apps().iter().map(|app| (app.app_id(), app.events())) {
        let resp_app = response
            .apps()
            .iter()
            .find(|a| a.app_id() == app)
            .ok_or_else(|| {
                HarpoonError::InvalidResponse(format!("Missing app '{}' in response", app))
            })?;

        if events.len() != resp_app.events().len() {
            return Err(HarpoonError::InvalidResponse(format!(
                "Expected {} events for app '{}', got {}",
                events.len(),
                app,
                resp_app.events().len()
            )));
        }

        for (request_event, response_event) in events.iter().zip(resp_app.events().iter()) {
            if !response_event.is_ok() {
                return Err(HarpoonError::EventNotAcknowledged(
                    request_event.event_type,
                    request_event.event_result,
                ));
            }
        }
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    use event::{OmahaEvent, OmahaEventType};
    use request::AppRequest;

    use crate::EventResult;

    #[test]
    fn test_send() {
        // Request a new server from the pool
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                </response>"#})
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request = Request::default();

        let response = send(&url, &request).unwrap();
        assert_eq!(response.apps().len(), 0);

        omaha_mock.assert();
    }

    #[test]
    fn test_send_event() {
        // Request a new server from the pool
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="app_id" status="ok">
                        <event status="ok"/>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request = Request::default().with_app(
            AppRequest::new_event("app_id", "track")
                .unwrap()
                .with_event(OmahaEvent::new(
                    OmahaEventType::EventUpdateInstalled,
                    EventResult::Success,
                )),
        );

        let response = send_event(&url, &request).unwrap();
        assert_eq!(response.apps().len(), 1);

        omaha_mock.assert();
    }

    #[test]
    fn test_send_event_reply_missing() {
        // Request a new server from the pool
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="app_id" status="ok">
                    </app>
                </response>"#})
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request = Request::default().with_app(
            AppRequest::new_event("app_id", "track")
                .unwrap()
                .with_event(OmahaEvent::new(
                    OmahaEventType::EventUpdateInstalled,
                    EventResult::Success,
                )),
        );

        let err = send_event(&url, &request).unwrap_err();
        assert_eq!(
            err,
            HarpoonError::InvalidResponse("Expected 1 events for app 'app_id', got 0".into())
        );

        omaha_mock.assert();
    }

    #[test]
    fn test_send_event_unacknowledged() {
        // Request a new server from the pool
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="app_id" status="ok">
                        <event status="error"/>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request = Request::default().with_app(
            AppRequest::new_event("app_id", "track")
                .unwrap()
                .with_event(OmahaEvent::new(
                    OmahaEventType::EventUpdateInstalled,
                    EventResult::Success,
                )),
        );

        let err = send_event(&url, &request).unwrap_err();
        assert_eq!(
            err,
            HarpoonError::EventNotAcknowledged(
                OmahaEventType::EventUpdateInstalled,
                EventResult::Success
            )
        );

        omaha_mock.assert();
    }
}
