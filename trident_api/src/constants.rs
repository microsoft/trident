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

/// Name of the overlay filesystem.
pub const OVERLAY_FILESYSTEM: &str = "overlay";

/// None/null mount point.
pub const NONE_MOUNT_POINT: &str = "none";

/// Swap mount point.
pub const SWAP_MOUNT_POINT: &str = NONE_MOUNT_POINT;

/// Datastore file extension.
pub const DATASTORE_FILE_EXTENSION: &str = "sqlite";

/// Default Trident datastore path. Used from the runtime OS.
pub const TRIDENT_DATASTORE_PATH_DEFAULT: &str = "/var/lib/trident/datastore.sqlite";

/// Path to load the agent config from.
pub const AGENT_CONFIG_PATH: &str = "/etc/trident/trident.conf";

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

/// The path to the root of the freshly deployed (from provisioning OS) or
/// updated OS (from runtime OS).
pub const UPDATE_ROOT_PATH: &str = "/mnt/newroot";

/// Absolute path to /usr directory.
pub const USR_MOUNT_POINT_PATH: &str = "/usr";

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

/// Hardcoded path to the location to store backing of the /etc overlayfs.
/// Expected to be on top of A/B update volume pair. Relative to root mount point.
pub const TRIDENT_OVERLAY_RELATIVE_PATH: &str = "var/lib/trident-overlay";

/// Hardcoded path to the location to store backing of the /etc overlayfs.
/// Expected to be on top of A/B update volume pair.
pub const TRIDENT_OVERLAY_PATH: &str = formatcp!("/{TRIDENT_OVERLAY_RELATIVE_PATH}");

/// The path to the root of the freshly deployed (from provisioning OS) or
/// updated OS (from runtime OS). To be used when /mnt/newroot is not available.
pub const UPDATE_ROOT_FALLBACK_PATH: &str = formatcp!("{TRIDENT_OVERLAY_PATH}/newroot");

/// Path to the mountinfo file in the host's proc directory that contains information about the
/// host's mount points.
pub const PROC_MOUNTINFO_PATH: &str = "/proc/self/mountinfo";

// /etc overlay related path constants

/// Lower directory relative path (etc).
pub const TRIDENT_OVERLAY_LOWER_RELATIVE_PATH: &str = "etc";

/// Work directory relative path (work).
pub const TRIDENT_OVERLAY_WORK_RELATIVE_PATH: &str = "etc/work";

/// Upper directory relative path (upper).
pub const TRIDENT_OVERLAY_UPPER_RELATIVE_PATH: &str = "etc/upper";

/// Dev Mapper path
pub const DEV_MAPPER_PATH: &str = "/dev/mapper";

/// Dev MD path
pub const DEV_MD_PATH: &str = "/dev/md";

/// Selinux config file path
pub const SELINUX_CONFIG: &str = "/etc/selinux/config";

/// /var/tmp path
pub const VAR_TMP_PATH: &str = "/var/tmp";

/// /proc/mdstat path
pub const MDSTAT_PATH: &str = "/proc/mdstat";

// Verity related constants

/// Root verity device name.
pub const ROOT_VERITY_DEVICE_NAME: &str = "root";

/// Usr verity device name.
pub const USR_VERITY_DEVICE_NAME: &str = "usr";

// OS/System Constants

/// Reduction in data device size when LUKS2 encryption is initialized.
pub const LUKS_HEADER_SIZE_IN_MIB: u32 = 16;

// Azure Linux Specific Constants

/// Azure Linux Install ID Prefix
pub const AZURE_LINUX_INSTALL_ID_PREFIX: &str = "AZL";

/// A/B Volume A Name
pub const AB_VOLUME_A_NAME: &str = "A";

/// A/B Volume B Name
pub const AB_VOLUME_B_NAME: &str = "B";

/// Read-only mount option.
pub const MOUNT_OPTION_READ_ONLY: &str = "ro";

/// Internal-only overrides
pub mod internal_params {
    /// Allow unused images in a COSI file.
    pub const ALLOW_UNUSED_FILESYSTEMS_IN_COSI: &str = "allowUnusedFilesystems";

    /// Disable check that filesystem size does not exceed the size of its block device.
    pub const DISABLE_FS_BLOCK_DEVICE_SIZE_CHECK: &str = "disableFsBlockDeviceSizeCheck";

    /// Disable check for grub-noprefix
    pub const DISABLE_GRUB_NOPREFIX_CHECK: &str = "disableGrubNoprefixCheck";

    /// Do not carry over existing machine hostname into the chroot during A/B update.
    pub const DISABLE_HOSTNAME_CARRY_OVER: &str = "disableHostnameCarryOver";

    /// Enable support for Harpoon to query for updated Host Config documents.
    pub const ENABLE_HARPOON_SUPPORT: &str = "harpoon";

    /// Experimental support for UKIs
    pub const ENABLE_UKI_SUPPORT: &str = "uki";

    /// Block Trident from closing encrypted volumes at the start of provisioning
    pub const NO_CLOSE_ENCRYPTED_VOLUMES: &str = "noCloseEncryptedVolumes";

    /// Block Trident from transitioning to the new OS after finalizing
    pub const NO_TRANSITION: &str = "noTransition";

    /// Allow configuration of orchestrator connection timeout
    pub const ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS: &str =
        "orchestratorConnectionTimeoutSeconds";

    /// Overrides the pcrlock encryption logic to use the previous logic where encryption volumes
    /// are sealed against a set of specific PCR values.
    pub const OVERRIDE_PCRLOCK_ENCRYPTION: &str = "overridePcrlockEncryption";

    /// Run extra partition and filesystem checks before reboot
    pub const PRE_REBOOT_CHECKS: &str = "preRebootChecks";

    /// Re-encrypt the encrypted LUKS2 volumes in-place on clean install, instead of initializing
    /// new LUKS2 volumes.
    pub const REENCRYPT_ON_CLEAN_INSTALL: &str = "reencryptOnCleanInstall";

    /// Relax COSI filesystem match checks.
    pub const RELAXED_COSI_VALIDATION: &str = "relaxedCosiValidation";

    /// Set the in-image paths of the verity signature files.
    ///
    /// The param MUST be a mapping of: Verity Block Device Id -> Absolute path
    /// of the signature file in the image.
    ///
    /// Example:
    ///
    /// ```yaml
    /// internalParams:
    ///   veritySignaturePaths:
    ///     usr: /boot/usr.hash.sig
    /// ```
    pub const VERITY_SIGNATURE_PATHS: &str = "veritySignaturePaths";

    /// Use alternate boot order logic to work around virtdeploy limitations.
    pub const VIRTDEPLOY_BOOT_ORDER_WORKAROUND: &str = "virtdeployBootOrderWorkaround";

    /// Force Trident to wait for systemd-networkd-wait-online
    pub const WAIT_FOR_SYSTEMD_NETWORKD: &str = "waitForSystemdNetworkd";

    /// Mount a writable overlay for /etc for the hooks subsystem.
    pub const WRITABLE_ETC_OVERLAY_HOOKS: &str = "writableEtcOverlayHooks";
}
