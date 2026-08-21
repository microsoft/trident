//! # trident-acl-agent
//!
//! trident-acl-agent is Trident's ACL update sidecar. Historically it was a
//! one-shot Omaha client that called Trident's combined `Update()` RPC once
//! and exited. This crate now defaults to the Kubernetes annotation protocol
//! described in the accepted design
//! (<https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GCeb7e534b2415ad52b37ef22fd49685e81e56c8aa&path=/docs/update-trigger-design.md>),
//! while preserving the original `omaha-only` mode as an explicit opt-out
//! (see `core::config::GoalSource`).
//!
//! All Omaha/Nebraska protocol traffic (both `omaha-only` and annotation mode)
//! goes through the [`core::nebraska`] client module, a self-contained,
//! reusable implementation of the Nebraska/Omaha update protocol. It is usable
//! both by this crate's agent binary and by a future Trident ACL Agent that
//! orchestrates updates differently.
//!
//! - [`core`]: building blocks shared by both modes (config, errors,
//!   machine-id, current-version, the `tridentd` client, the Nebraska
//!   client).
//! - [`annotations`]: the default Kubernetes annotation-driven protocol.
//! - [`omahaonly`]: the legacy one-shot Omaha flow.

pub mod annotations;
pub mod core;
pub mod omahaonly;

/// The version this agent reports to Nebraska as the updater's own version, for
/// [`core::nebraska::Client::new`].
///
/// Prefers the build-time `TRIDENT_VERSION` (the version the shipped product is
/// stamped with) over this crate's package version, which is not released
/// independently and is a placeholder. Nebraska itself ignores the value, so
/// this is for whoever reads the raw requests. It lives here, not in
/// [`core::nebraska`], because that module is a generic Omaha client: which
/// product is doing the updating is the caller's business.
pub const AGENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

use crate::core::error::AgentError;
use crate::core::nebraska::{Client, MachineId, NebraskaError};

pub use crate::core::id::IdSource;

// Deliberately invalid sentinels, mirroring DEFAULT_NEBRASKA_ENDPOINT's
// `.invalid` domain trick: a deployment that forgets to configure (or
// override via the update-request annotation's `appId`/`track` fields) a
// real app_id/track fails loudly against Nebraska instead of silently
// querying a real-looking but wrong app/group.
pub const DEFAULT_NEBRASKA_APP_ID: &str = "00000000-0000-0000-0000-000000000000";
pub const DEFAULT_NEBRASKA_TRACK: &str = "unspecified";

/// Builds a validated [`MachineId`] from an [`IdSource`], translating the
/// crate's own machine-id/hostname read errors into a single [`AgentError`].
fn build_machine_id(source: IdSource) -> Result<MachineId, AgentError> {
    MachineId::new(source.produce_id()?).map_err(|err| AgentError::Nebraska(err.to_string()))
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
) -> Result<(), AgentError> {
    let machine_id = build_machine_id(machine_id_source)?;
    let client = Client::new(url.clone(), app_id, track, machine_id);
    match client.check_for_update(&semver::Version::new(0, 0, 0)) {
        Ok(_) => Ok(()),
        // A well-formed response reporting a non-OK app/update-check status
        // still proves the server is reachable and speaking Omaha; only a
        // transport/parse-level failure means it is not.
        Err(NebraskaError::ServerError(_)) => Ok(()),
        Err(err) => Err(AgentError::Nebraska(err.to_string())),
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
        assert!(matches!(err, AgentError::Nebraska(_)));
    }
}
