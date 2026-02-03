use super::metadata::MetadataVersion;

#[derive(thiserror::Error, Debug, Clone, Eq, PartialEq)]
#[error("Invalid COSI metadata v{version}: {kind}")]
pub struct CosiMetadataError {
    pub version: MetadataVersion,
    pub kind: CosiMetadataErrorKind,
}

#[derive(thiserror::Error, Debug, Clone, Eq, PartialEq)]
pub enum CosiMetadataErrorKind {
    #[error("Duplicate mount point: '{0}'")]
    V1_0DuplicateMountPoint(String),

    #[error("Bootloader metadata is required for COSI version >= 1.1, but not provided")]
    V1_1BootloaderRequired,

    #[error("Bootloader type 'grub' cannot have systemd-boot entries")]
    V1_1GrubWithSystemdBootEntries,

    #[error("Bootloader type 'systemd-boot' requires systemd-boot entries")]
    V1_1SystemdBootMissingEntries,

    #[error("Bootloader type 'systemd-boot' must not be empty")]
    V1_1SystemdBootEmptyEntries,

    #[error("OS packages metadata is required for COSI version >= 1.1, but not provided")]
    V1_1OsPackagesRequired,

    #[error("OS package '{0}' is missing required release information")]
    V1_1OsPackageMissingRelease(String),

    #[error("OS package '{0}' is missing required architecture information")]
    V1_1OsPackageMissingArch(String),

    #[error("Image disk metadata is required for COSI version >= 1.2, but not provided")]
    V1_2DiskInfoRequired,

    #[error("Disk regions array is empty")]
    V1_2DiskRegionsMissing,

    #[error("First disk region in metadata must be the primary GPT at LBA 0, found region '{region_type}' at LBA {lba}")]
    V1_2DiskRegionsInvalidFirstRegion { region_type: String, lba: u64 },

    #[error("Disk partition table type must be GPT, found '{0}'")]
    V1_2DiskPartitionTableNotGpt(String),

    #[error("Duplicate partition number: {0}")]
    V1_2DuplicatePartitionNumber(u32),

    #[error("Partition numbers must be 1-indexed; found partition number 0")]
    V1_2PartitionNumberZero,

    #[error("Image at path '{path}' has different '{field}' in the disk and filesystem sections, disk: '{disk_image}', filesystem: '{fs_image}'")]
    V1_2ImageFileMetadataMismatch {
        path: String,
        field: String,
        disk_image: String,
        fs_image: String,
    },

    #[error("Image file at path '{0}' has no corresponding partition")]
    V1_2ImageFileHasNoCorrespondingPartition(String),
}
