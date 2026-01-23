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

    #[error("Image partition metadata is required for COSI version >= 1.2, but not provided")]
    V1_2PartitionsRequired,

    #[error(
        "Partition {number} references path '{path}' which does not match any filesystem image"
    )]
    V1_2PartitionPathUnknown { number: u32, path: String },

    #[error("Duplicate partition number: {0}")]
    V1_2DuplicatePartitionNumber(u32),

    #[error("Partition numbers must be 1-indexed; found partition number 0")]
    V1_2PartitionNumberZero,
}
