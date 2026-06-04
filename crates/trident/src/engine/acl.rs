//! ACL (Azure Container Linux) UKI-specific constants and helpers.
//!
//! ACL uses fixed PARTUUIDs for USR A/B partitions and a verity addon that
//! places the root hash in the kernel command line as `usrhash=<hex>`.

use std::fs;

// ACL UKI disk layout defines fixed PARTUUIDs for the USR A/B data partitions.
// These are from acl-scripts disk_layout_uki.json.
pub const ACL_USR_A_PARTUUID: &str = "7130c94a-213a-4e5a-8e26-6cce9662f132";
pub const ACL_USR_B_PARTUUID: &str = "e03dd35c-7c2d-4a47-b3fe-27f15780a57c";

/// Reads the active USR verity root hash from `/proc/cmdline`.
///
/// ACL UKI images include a `usrhash=<hex>` parameter in the kernel command
/// line (contributed by the verity addon). Returns `None` if the parameter
/// is not present or `/proc/cmdline` cannot be read.
pub fn read_active_usr_roothash() -> Option<String> {
    let cmdline = fs::read_to_string("/proc/cmdline").ok()?;
    cmdline
        .split_whitespace()
        .find_map(|field| field.strip_prefix("usrhash="))
        .map(|hash| hash.to_owned())
}
