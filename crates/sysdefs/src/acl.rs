//! Azure Container Linux (ACL) system definitions.
//!
//! Fixed PARTUUIDs and partition type UUIDs for the ACL UKI disk layout,
//! sourced from acl-scripts disk_layout_uki.json.

use uuid::{uuid, Uuid};

/// ACL USR partition A PARTUUID.
pub const ACL_USR_A_PARTUUID: Uuid = uuid!("7130c94a-213a-4e5a-8e26-6cce9662f132");

/// ACL USR partition B PARTUUID.
pub const ACL_USR_B_PARTUUID: Uuid = uuid!("e03dd35c-7c2d-4a47-b3fe-27f15780a57c");

/// ACL USR partition type UUID.
pub const ACL_USR_PARTITION_TYPE_UUID: Uuid = uuid!("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6");
