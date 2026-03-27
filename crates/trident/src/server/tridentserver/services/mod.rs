use trident_proto::v1::{RebootHandling, RebootManagement};

use crate::server::tridentserver::RebootDecision;

mod streaming;
mod version;

#[cfg(feature = "grpc-preview")]
mod commit;
#[cfg(feature = "grpc-preview")]
mod install;
#[cfg(feature = "grpc-preview")]
mod rebuild_raid;
#[cfg(feature = "grpc-preview")]
mod rollback;
#[cfg(feature = "grpc-preview")]
mod status;
#[cfg(feature = "grpc-preview")]
mod update;
#[cfg(feature = "grpc-preview")]
mod validation;

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
