// Configuration constants

/// Size of a partition that will be grown to fill all available space.
pub const PARTITION_SIZE_GROW: &str = "grow";

/// Default interpreter to use for scripts if not specified.
pub const DEFAULT_SCRIPT_INTERPRETER: &str = "/bin/sh";

/// Ignore the checksum of the image.
pub const IMAGE_SHA256_CHECKSUM_IGNORED: &str = "ignored";

/// Name of the swap filesystem.
pub const SWAP_FILESYSTEM: &str = "swap";

/// None/null mount point.
pub const NONE_MOUNT_POINT: &str = "none";

/// Swap mount point.
pub const SWAP_MOUNT_POINT: &str = NONE_MOUNT_POINT;

/// ESP partition mount point path.
pub const ESP_MOUNT_POINT_PATH: &str = "/boot/efi";
