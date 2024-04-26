mod host;
mod local;

pub use host::{
    error::InvalidHostConfigurationError,
    os::{
        additional_files::AdditionalFile,
        users::{Password, SshMode, User},
        ManagementOs, Os,
    },
    scripts::{Script, Scripts, ServicingType},
    storage::imaging::{AbUpdate, AbVolumePair, Image, ImageFormat, ImageSha256},
    storage::{
        disks::{Disk, PartitionTableType},
        encryption::{EncryptedVolume, Encryption},
        filesystem::{
            FileSystem, FileSystemSource, FileSystemType, MountOptions, MountPoint, MountPointInfo,
            VerityFileSystem,
        },
        internal::{InternalImage, InternalMountPoint, InternalVerityDevice},
        partitions::{AdoptedPartition, Partition, PartitionSize, PartitionType},
        raid::{Raid, RaidLevel, SoftwareRaidArray},
        Storage,
    },
    trident::Trident,
    HostConfiguration,
};

pub use local::{GrpcConfiguration, HostConfigurationSource, LocalConfigFile, Operations};
