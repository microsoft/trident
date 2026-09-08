use tonic::Status;
use trident_api::error::{InvalidInputError, TridentError};
use trident_proto::v1::{RebootHandling, RebootManagement};

use crate::{
    logging::operation_context,
    server::tridentserver::{RebootDecision, TridentServer},
};

mod commit;
mod rollback;
mod streaming;
mod update;
mod version;

#[cfg(feature = "grpc-preview")]
mod install;
#[cfg(feature = "grpc-preview")]
mod rebuild_raid;
#[cfg(feature = "grpc-preview")]
mod status;
#[cfg(feature = "grpc-preview")]
mod validation;

impl TridentServer {
    /// Rejects a gRPC request whose payload failed pre-dispatch validation
    /// (missing field, unparsable Host Configuration, etc.) before
    /// `servicing_request` ever runs. Without this, a rejected request left
    /// no telemetry trace at all -- no `command_start`/`command_error` --
    /// unlike every request that makes it far enough to be serviced. Fires
    /// both metrics for a synthetic, immediately-failed operation tagged with
    /// the same `command` name the real dispatch would have used, then
    /// returns the `Status` to send back to the caller.
    ///
    /// Calls `refresh_ids` first, exactly like
    /// `servicing_request`/`reading_request` do, so a rejected request
    /// still gets the host's installation ID attached when one is
    /// available. Without this, `command_start`/`command_error` for every
    /// rejected request went out with no installation ID at all, even on
    /// an already-provisioned host -- unlike a request that makes it far
    /// enough to be serviced, which always calls `refresh_ids`
    /// via `servicing_request`/`reading_request`.
    ///
    /// `#[track_caller]` so `TridentError::new` below (itself
    /// `#[track_caller]`) attributes this error's `location` to whichever
    /// service handler actually rejected the request, not to this shared
    /// helper's own line -- otherwise every rejection from every RPC would
    /// report the identical, uninformative `location`.
    #[track_caller]
    fn reject_invalid_argument(
        &self,
        command: &str,
        field: &str,
        message: impl Into<String>,
    ) -> Status {
        self.refresh_ids();
        let error = TridentError::new(InvalidInputError::MissingRequestField {
            field: field.to_owned(),
        });
        // Unlike `servicing_request`'s closures, this runs directly on the
        // async gRPC handler's Tokio worker thread, not inside
        // `spawn_blocking` -- but `run_command` still synchronously fires
        // tracing events, and a configured remote telemetry sender does a
        // blocking `reqwest::blocking` POST from inside that same call
        // (`TraceSender::on_event`). `block_in_place` tells the multi-threaded
        // runtime this thread is about to block, so it can hand off other
        // queued work to another worker instead of stalling behind it -- a
        // burst of malformed requests can no longer starve the runtime.
        let _ = tokio::task::block_in_place(|| {
            operation_context::run_command(
                command,
                operation_context::OperationSource::Daemon,
                || Err::<(), _>(error),
            )
        });
        Status::invalid_argument(message.into())
    }

    /// Same as [`Self::reject_invalid_argument`], but for a field that is
    /// present yet fails to parse or otherwise doesn't satisfy the
    /// request's requirements (e.g. `stream_disk`'s image URL) rather than
    /// a missing field.
    ///
    /// `#[track_caller]` for the same reason as
    /// [`Self::reject_invalid_argument`].
    #[track_caller]
    fn reject_invalid_field(
        &self,
        command: &str,
        field: &str,
        reason: impl Into<String>,
        message: impl Into<String>,
    ) -> Status {
        self.refresh_ids();
        let error = TridentError::new(InvalidInputError::InvalidRequestField {
            field: field.to_owned(),
            reason: reason.into(),
        });
        // See the `block_in_place` comment in `reject_invalid_argument`.
        let _ = tokio::task::block_in_place(|| {
            operation_context::run_command(
                command,
                operation_context::OperationSource::Daemon,
                || Err::<(), _>(error),
            )
        });
        Status::invalid_argument(message.into())
    }
}

/// Returns a `RebootDecision` indicating whether Trident can perform a reboot
/// given a provided optional RebootManagement struct.
fn reboot_allowed(reboot_opt: &Option<RebootManagement>) -> RebootDecision {
    if let Some(reboot) = reboot_opt {
        match reboot.handling() {
            // On unspecified, assume that Trident can handle the reboot, as
            // that is the safest option.
            RebootHandling::Unspecified => RebootDecision::Handle,

            // The caller explicitly specified that Trident can handle reboots,
            // so we allow it.
            RebootHandling::TridentHandlesReboot => RebootDecision::Handle,

            // The caller explicitly specified that they will handle reboots, so
            // we defer to them.
            RebootHandling::CallerHandlesReboot => RebootDecision::Defer,
        }
    } else {
        // If no reboot configuration is provided, we default to Trident
        // handling reboots.
        RebootDecision::Handle
    }
}
