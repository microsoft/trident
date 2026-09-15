//! # trident-acl-agent
//!
//! trident-acl-agent is Trident's ACL update sidecar. By default it drives
//! Trident (stage/finalize/rollback/commit) through a Kubernetes node
//! annotation protocol ([`annotations`]); a one-shot `omaha-only` mode
//! ([`omahaonly`]) that calls Trident's combined `Update()` RPC once and
//! exits is also available as an explicit opt-out (see
//! `core::config::Mode`). Building blocks shared by both modes live in
//! [`core`].
//!
//! All Omaha/Nebraska protocol traffic (both `omaha-only` and annotation mode)
//! goes through the [`core::nebraska`] client module, a self-contained,
//! reusable implementation of the Nebraska/Omaha update protocol. It is usable
//! both by this crate's agent binary and by a future Trident ACL Agent that
//! orchestrates updates differently.

use semver::Version;
use url::Url;

use crate::core::{
    error::AgentError,
    nebraska::{Client, MachineId, NebraskaError},
    version::FALLBACK_ALWAYS_VERSION,
};

pub use crate::core::{id::IdSource, nebraska};

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
/// non-OK app status) as a failure. Unlike [`Client::check_for_update`], this
/// only fails on network/transport problems or a response that isn't
/// well-formed Omaha XML -- it's meant for a pure "can we talk to this server
/// at all" check (e.g. `--validate-connection nebraska`), not for deciding
/// whether an update is available.
///
/// Uses [`Client::ping`], not `check_for_update`: against a real, stateful
/// Nebraska, `check_for_update` has the side effect of registering an update
/// check for this machine id, which Nebraska can then grant -- consuming the
/// update and leaving the instance "in progress" for a real stage/finalize
/// request that reuses the same machine id. `ping` sends a bare Omaha
/// `<ping/>` with no `<updatecheck/>`, so this check proves reachability
/// without any such side effect.
pub fn check_nebraska_reachable(
    url: &Url,
    app_id: &str,
    track: &str,
    machine_id_source: IdSource,
) -> Result<(), AgentError> {
    let machine_id = build_machine_id(machine_id_source)?;
    let client = Client::new(url.clone(), app_id, track, machine_id);
    match client.ping(
        &Version::parse(FALLBACK_ALWAYS_VERSION)
            .expect("invariant: FALLBACK_ALWAYS_VERSION is valid semver"),
    ) {
        Ok(()) => Ok(()),
        // A well-formed response reporting a non-OK app status still proves
        // the server is reachable and speaking Omaha; only a
        // transport/parse-level failure means it is not.
        Err(NebraskaError::ServerError(_)) => Ok(()),
        Err(err) => Err(AgentError::Nebraska(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use mockito::{Matcher, Server};
    use url::Url;

    #[test]
    fn test_check_nebraska_reachable_never_sends_an_update_check() {
        // Regression test: check_nebraska_reachable() must use Client::ping(),
        // not check_for_update() -- against a real, stateful Nebraska,
        // check_for_update() has the side effect of granting/consuming an
        // update for this machine id, which a diagnostic connectivity check
        // must never do.
        let mut server = Server::new();

        let ping_mock = server
            .mock("POST", "/")
            .match_body(Matcher::Regex("<ping".to_string()))
            .with_status(200)
            .with_body(indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <ping status="ok"></ping>
                    </app>
                </response>"#})
            .expect(1)
            .create();
        let no_update_check_mock = server
            .mock("POST", "/")
            .match_body(Matcher::Regex("<updatecheck".to_string()))
            .expect(0)
            .create();

        check_nebraska_reachable(
            &Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();

        ping_mock.assert();
        no_update_check_mock.assert();
    }

    #[test]
    fn test_check_nebraska_reachable_succeeds_on_error_app_status() {
        // check_nebraska_reachable() is meant to be a pure "can we reach this
        // server and does it speak Omaha" check: a well-formed response with
        // a non-OK app status should still count as "reachable" here.
        let mut server = Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="error-unknownApplication">
                        <ping status="ok"></ping>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        check_nebraska_reachable(
            &Url::parse(&server.url()).unwrap(),
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
            &Url::parse("http://127.0.0.1:0/").unwrap(),
            "test",
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap_err();
        assert!(matches!(err, AgentError::Nebraska(_)));
    }
}
