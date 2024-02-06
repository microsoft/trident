use const_format::formatcp;

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

// Block of volume agnostic path constants

/// Boot directory name.
pub const BOOT_DIRECTORY: &str = "boot";

/// GRUB2 directory name.
pub const GRUB2_DIRECTORY: &str = "grub2";

/// GRUB2 directory relative path (boot/grub2). This is the default location for
/// GRUB config.
pub const GRUB2_RELATIVE_PATH: &str = formatcp!("{BOOT_DIRECTORY}/{GRUB2_DIRECTORY}");

/// GRUB2 configuration file name.
pub const GRUB2_CONFIG_FILENAME: &str = "grub.cfg";

/// GRUB2 configuration file path (boot/grub2/grub.cfg).
pub const GRUB2_CONFIG_RELATIVE_PATH: &str =
    formatcp!("{GRUB2_RELATIVE_PATH}/{GRUB2_CONFIG_FILENAME}");

// Block of ESP specific path constants

/// EFI directory name.
pub const ESP_EFI_DIRECTORY: &str = "EFI";

/// BOOT directory name.
pub const EFI_DEFAULT_BIN_DIRECTORY: &str = "BOOT";

/// BOOT directory relative path (EFI/BOOT) to the ESP mount point. This is the
/// fallback location for the EFI boot loader.
pub const EFI_DEFAULT_BIN_RELATIVE_PATH: &str =
    formatcp!("{ESP_EFI_DIRECTORY}/{EFI_DEFAULT_BIN_DIRECTORY}");

// Block of root specific path contants

/// efi directory name.
pub const ROOT_EFI_DIRECTORY: &str = "efi";

// Block of path constants specific to mount points

/// Root volume mount point path.
pub const ROOT_MOUNT_POINT_PATH: &str = "/";

/// Boot volume relative mount point path (boot) relative to the root mount point.
pub const BOOT_RELATIVE_MOUNT_POINT_PATH: &str = BOOT_DIRECTORY;

/// Boot volume mount point path (/boot).
pub const BOOT_MOUNT_POINT_PATH: &str =
    formatcp!("{ROOT_MOUNT_POINT_PATH}{BOOT_RELATIVE_MOUNT_POINT_PATH}",);

/// ESP volume relative mount point path (boot/efi) relative to the root mount point.
pub const ESP_RELATIVE_MOUNT_POINT_PATH: &str = formatcp!("{BOOT_DIRECTORY}/{ROOT_EFI_DIRECTORY}");

/// ESP volume mount point path (/boot/efi).
pub const ESP_MOUNT_POINT_PATH: &str =
    formatcp!("{ROOT_MOUNT_POINT_PATH}{ESP_RELATIVE_MOUNT_POINT_PATH}");
