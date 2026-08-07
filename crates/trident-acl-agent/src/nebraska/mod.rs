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
//! invariants in the type system so they cannot be violated by accident:
//!
//! - **Only six `(eventtype, eventresult)` pairs are accepted** by Nebraska;
//!   any other pair is silently discarded (the server still returns
//!   `<event status="ok">`). Raw integers never appear in the public API — see
//!   [`ProgressEvent`] and the private wire mapping.
//! - **`track` is mandatory on every request**, including event-only ones:
//!   Nebraska resolves the group from `track` before processing events, so
//!   omitting it silently drops them. It is a field of [`Client`], so it cannot
//!   be omitted.
//! - **`error-updateInProgressOnInstance` is expected, not fatal.** Nebraska
//!   returns it on every update check between the first progress event and the
//!   terminal one; it is modelled as [`CheckOutcome::UpdateInProgress`], and
//!   unknown status strings never break parsing (see [`AppStatus`]).
//! - **The machine id must be unbraced and stable.** Nebraska filters
//!   brace-wrapped ids out of its UI and statistics, and uses the id as the
//!   instance primary key. See [`MachineId`].
//! - **Event reporting is all-or-nothing.** Sending progress events commits the
//!   caller to a terminal event that fires after a reboot; the terminal
//!   operations are dedicated methods on [`Client`] rather than free-standing
//!   values.
//!
//! # Example
//!
//! ```no_run
//! use semver::Version;
//! use url::Url;
//! use trident_acl_agent::nebraska::{Client, CheckOutcome, MachineId, ProgressEvent};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::new(
//!     Url::parse("https://updates.example.com/v1/update/")?,
//!     "example-app",
//!     "stable",
//!     MachineId::from_uuid(uuid::Uuid::new_v4()),
//! );
//!
//! let current = Version::new(1, 0, 0);
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

pub use client::{CheckOutcome, Client, PackageHash, UpdateOffer};
pub use error::NebraskaError;
pub use event::ProgressEvent;
pub use id::MachineId;
pub use status::{AppStatus, UpdateCheckStatus};
pub use transport::{ReqwestTransport, Transport};
