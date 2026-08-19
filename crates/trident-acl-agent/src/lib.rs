//! Library surface for the `trident-acl-agent` crate.
//!
//! Currently this exposes the [`nebraska`] client module, a self-contained,
//! reusable implementation of the Nebraska/Omaha update protocol. It is usable
//! both by this crate's agent binary and by a future Trident ACL Agent that
//! orchestrates updates differently.

pub mod nebraska;

/// The version this agent reports to Nebraska as the updater's own version, for
/// [`nebraska::Client::new`].
///
/// Prefers the build-time `TRIDENT_VERSION` (the version the shipped product is
/// stamped with) over this crate's package version, which is not released
/// independently and would report a placeholder to operators. This lives here,
/// not in [`nebraska`], because that module is a generic Omaha client: which
/// product is doing the updating is the caller's business.
pub const AGENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
