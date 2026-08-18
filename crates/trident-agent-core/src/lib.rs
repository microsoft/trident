//! # trident-agent-core
//!
//! Shared building blocks for Trident's node-agent binaries:
//! [`trident-acl-agent`](../trident_acl_agent) (one-shot Omaha/Nebraska
//! client, no Kubernetes) and
//! [`trident-aks-agent`](../trident_aks_agent) (AKS annotation-driven
//! orchestrator). Both link this
//! crate for:
//!
//! - [`nebraska`] - a self-contained Nebraska/Omaha protocol client.
//! - [`trident`] - a gRPC client wrapper for tridentd's stable v1
//!   Update/Commit/Rollback services.
//! - [`id`] - machine-id/hostname identity helpers for Nebraska requests.
//! - [`error`] - the shared [`AgentCoreError`] type.
//!
//! Kubernetes access, the annotation wire protocol, and the persisted
//! `state.json` are all specific to the annotation-driven orchestrator and
//! live in `trident-aks-agent` instead.

use semver::Version;

pub mod config;
pub mod error;
pub mod id;
pub mod nebraska;
pub mod trident;

use error::AgentCoreError;
use nebraska::{Client, MachineId, NebraskaError};

pub use id::IdSource;

// Deliberately invalid sentinels: a deployment that forgets to configure
// (or override, per-request) a real app_id/track fails loudly against
// Nebraska instead of silently querying a real-looking but wrong app/group.
pub const DEFAULT_NEBRASKA_APP_ID: &str = "00000000-0000-0000-0000-000000000000";
pub const DEFAULT_NEBRASKA_TRACK: &str = "unspecified";

/// Builds a validated [`MachineId`] from an [`IdSource`], translating the
/// crate's own machine-id/hostname read errors into a single
/// [`AgentCoreError`].
pub fn build_machine_id(source: IdSource) -> Result<MachineId, AgentCoreError> {
    MachineId::new(source.produce_id()?).map_err(|err| AgentCoreError::Nebraska(err.to_string()))
}

/// Checks that the Omaha/Nebraska server at `url` is reachable and speaking
/// the Omaha protocol, without treating any app-level result (including a
/// non-OK app/update-check status) as a failure. Unlike
/// [`Client::check_for_update`], this only fails on network/transport
/// problems or a response that isn't well-formed Omaha XML -- it's meant for
/// a pure "can we talk to this server at all" check (e.g.
/// `--validate-connection nebraska`), not for deciding whether an update is
/// available.
pub fn check_nebraska_reachable(
    url: &url::Url,
    app_id: &str,
    track: &str,
    machine_id_source: IdSource,
) -> Result<(), AgentCoreError> {
    let machine_id = build_machine_id(machine_id_source)?;
    let client = Client::new(url.clone(), app_id, track, machine_id);
    match client.check_for_update(&Version::new(0, 0, 0)) {
        Ok(_) => Ok(()),
        // A well-formed response reporting a non-OK app/update-check status
        // still proves the server is reachable and speaking Omaha; only a
        // transport/parse-level failure means it is not.
        Err(NebraskaError::ServerError(_)) => Ok(()),
        Err(err) => Err(AgentCoreError::Nebraska(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_nebraska_reachable_succeeds_on_error_app_status() {
        // check_nebraska_reachable() is meant to be a pure "can we reach this
        // server and does it speak Omaha" check, unlike check_for_update()
        // which also validates app-level semantics. A well-formed response
        // with a non-OK app status should still count as "reachable" here,
        // even though check_for_update() would reject the same response as a
        // NebraskaError::ServerError.
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="error-unknownApplication">
                        <updatecheck status="error-internal"></updatecheck>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        check_nebraska_reachable(
            &url::Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();

        omaha_mock.assert();
    }

    #[test]
    fn test_check_nebraska_reachable_fails_on_transport_error() {
        let err = check_nebraska_reachable(
            // Port 0 never accepts a connection.
            &url::Url::parse("http://127.0.0.1:0/").unwrap(),
            "test",
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap_err();
        assert!(matches!(err, AgentCoreError::Nebraska(_)));
    }
}
