//! Azure Container Linux (ACL) system definitions.
//!
//! Fixed PARTUUIDs and partition type UUIDs for the ACL UKI disk layout,
//! sourced from acl-scripts disk_layout_uki.json, and the layout of the
//! `/etc` overlay ACL assembles at boot.

use uuid::{uuid, Uuid};

/// ACL USR partition A PARTUUID.
pub const ACL_USR_A_PARTUUID: Uuid = uuid!("7130c94a-213a-4e5a-8e26-6cce9662f132");

/// ACL USR partition B PARTUUID.
pub const ACL_USR_B_PARTUUID: Uuid = uuid!("e03dd35c-7c2d-4a47-b3fe-27f15780a57c");

/// ACL USR partition type UUID.
pub const ACL_USR_PARTITION_TYPE_UUID: Uuid = uuid!("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6");

/// Directory on the sealed `/usr` holding the factory contents of `/etc`.
///
/// ACL does not ship an `/etc` on its root filesystem. Its initrd instead
/// mounts an overlay over `/etc` whose lower layer is this directory and whose
/// upper layer is the root filesystem's own `/etc`, so the factory files are
/// visible while modifications land on the root filesystem. See the
/// `99setup-root` dracut module in the ACL image.
pub const ACL_ETC_LOWER_DIR: &str = "/usr/share/distro/etc";

/// Working directory the `/etc` overlay requires, on the root filesystem.
///
/// overlayfs requires the work directory to live on the same filesystem as the
/// upper layer. ACL's initrd uses this path for the same reason.
pub const ACL_ETC_WORK_DIR: &str = "/.etc-work";

/// Mount options ACL's initrd uses for the `/etc` overlay.
///
/// Matching them keeps copy-up behaving the same whether the overlay was
/// mounted by the initrd or by servicing.
pub const ACL_ETC_OVERLAY_OPTIONS: &str = "redirect_dir=on,metacopy=off";
