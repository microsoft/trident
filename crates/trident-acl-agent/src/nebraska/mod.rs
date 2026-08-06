//! A client for the [Nebraska](https://github.com/flatcar/nebraska) update
//! server, speaking the Omaha protocol.
//!
//! This module is scoped strictly to the Nebraska/Omaha protocol: building and
//! sending update checks and events, and interpreting the responses. It knows
//! nothing about Trident, reboots, commits, or the update orchestration around
//! it — that separation is deliberate.
//!
//! # Why the API looks the way it does
//!
//! The Omaha protocol as Nebraska implements it has several invariants that
//! **fail silently** when violated — a client can appear to work over the wire
//! while leaving the fleet's state permanently wrong. This module encodes those
//! invariants in the type system so they cannot be violated by accident. The
//! authoritative behavioural reference is the protocol spec at
//! `knowledge/topics/nebraska-client-protocol.md` in the `pacobot` repository;
//! the docs below cite its sections. In summary:
//!
//! - **Only six `(eventtype, eventresult)` pairs are accepted**; anything else
//!   is silently discarded. Raw integers never appear in the public API — see
//!   [`ProgressEvent`] and the private wire mapping (spec §2).
//! - **`track` is mandatory on every request**, including event-only ones. It
//!   is a field of [`Client`], so it cannot be omitted (spec §7 trap 4).
//! - **`error-updateInProgressOnInstance` is expected, not fatal.** It is
//!   modelled as [`CheckOutcome::UpdateInProgress`], and unknown status strings
//!   never break parsing (spec §4, §7 trap 1).
//! - **The machine id must be unbraced and stable.** See [`MachineId`]
//!   (spec §7 traps 2, 3).
//! - **Event reporting is all-or-nothing.** Sending progress events commits the
//!   caller to a terminal event that fires after a reboot; the terminal
//!   operations are dedicated methods on [`Client`] rather than free-standing
//!   values (spec §3).
//!
//! # Example
//!
//! ```no_run
//! use semver::Version;
//! use url::Url;
//! use trident_acl_agent::nebraska::{Client, CheckOutcome, MachineId, ProgressEvent};
//!
//! # fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::new(
//!     Url::parse("https://nebraska.example/v1/update/")?,
//!     "6d10cf97-443f-4542-8479-b9fdb44c9588",
//!     "stable",
//!     MachineId::from_uuid(uuid::Uuid::new_v4()),
//! );
//!
//! let current = Version::new(3, 0, 20260731);
//! match client.check_for_update(&current)? {
//!     CheckOutcome::UpToDate => {}
//!     CheckOutcome::UpdateInProgress => {}
//!     CheckOutcome::UpdateAvailable(offer) => {
//!         // (drive the update via Trident, out of this module's scope)
//!         client.report_progress(&current, ProgressEvent::DownloadStarted)?;
//!         // ... download finished, installed, then reboot ...
//!         // After the reboot, from a fresh process running the new version:
//!         client.complete_after_reboot(&current, &offer.version)?;
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
mod event;
mod id;
mod status;
mod transport;
mod wire;

pub use client::{CheckOutcome, Client, UpdateOffer};
pub use error::NebraskaError;
pub use event::ProgressEvent;
pub use id::MachineId;
pub use status::{AppStatus, UpdateCheckStatus};
pub use transport::{ReqwestTransport, Transport};
