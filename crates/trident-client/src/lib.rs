//! High-level asynchronous client for `tridentd`.
//!
//! The stable API is available by default. Preview RPCs are enabled with the
//! `preview` feature. Transport failures expose an [`ExecutionState`] so a
//! caller can distinguish work that was not started from work whose outcome is
//! unknown.
//!
//! Typed Host Configuration:
//! ```no_run
//! # use trident_client::{Client, HostConfiguration, RebootPolicy};
//! # async fn example() -> Result<(), trident_client::ClientError> {
//! let client = Client::connect_default().await?;
//! let config = HostConfiguration::default();
//! let completion = client
//!     .update(&config, RebootPolicy::Caller)
//!     .await?
//!     .wait()
//!     .await?;
//! println!("{:?}", completion.reboot);
//! # Ok(())
//! # }
//! ```
//!
//! Raw YAML is passed through unchanged for compatibility:
//! ```no_run
//! # use trident_client::{Client, RebootPolicy};
//! # async fn example() -> Result<(), trident_client::ClientError> {
//! let client = Client::connect_default().await?;
//! client
//!     .update_yaml("{}\n", RebootPolicy::Caller)
//!     .await?
//!     .wait()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Installer-style event handling:
//! ```no_run
//! # use trident_client::{
//! #     Client, ClientError, HostConfiguration, OperationOutcome, RebootPolicy,
//! #     ServicingEvent,
//! # };
//! # async fn example() -> Result<(), trident_client::ClientError> {
//! let client = Client::connect_default().await?;
//! let mut operation = client
//!     .update(&HostConfiguration::default(), RebootPolicy::Caller)
//!     .await?;
//! while let Some(event) = operation.next_event().await? {
//!     match event {
//!         ServicingEvent::Log(record) => println!("{}", record.message),
//!         ServicingEvent::Completed(OperationOutcome::Succeeded(_)) => return Ok(()),
//!         ServicingEvent::Completed(OperationOutcome::Failed(remote)) => {
//!             return Err(ClientError::RemoteFailure {
//!                 operation: operation.operation(),
//!                 remote,
//!             });
//!         }
//!         ServicingEvent::Started { .. } => {}
//!         _ => {}
//!     }
//! }
//! unreachable!("the client rejects streams without a completion event")
//! # }
//! ```

mod client;
mod error;
mod operation;
mod types;

#[cfg(feature = "preview")]
mod preview;

pub use url::Url;

pub use trident_api::{config::HostConfiguration, primitives::hash::Sha384Hash};

pub use client::{Client, ClientBuilder, DEFAULT_ENDPOINT};
pub use error::{ClientError, ExecutionState, RpcCode};
pub use operation::ServicingOperation;
pub use types::{
    Completion, EventTimestamp, ImageHash, LogLevel, LogRecord, Operation, OperationOutcome,
    RebootDisposition, RebootPolicy, RemoteError, RemoteErrorCategory, RollbackKind,
    ServicingEvent, ServicingKind, SourceLocation,
};

#[cfg(feature = "preview")]
pub use types::{ActiveVolume, RollbackAvailability, ServicingState, ValidationResult};

#[cfg(test)]
mod tests;
