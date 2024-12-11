pub(crate) mod host;
pub(crate) mod local;

pub use host::{
    error::{HostConfigurationDynamicValidationError, HostConfigurationStaticValidationError},
    harpoon::HarpoonConfig,
    image::{CosiFile, OsImage},
    os::{
        additional_files::AdditionalFile,
        modules::Module,
        services::Services,
        users::{Password, SshMode, User},
        KernelCommandLine, ManagementOs, Os, Selinux, SelinuxMode,
    },
    scripts::{Script, ScriptSource, Scripts, ServicingTypeSelection},
    storage::imaging::{AbUpdate, AbVolumePair, Image, ImageFormat, ImageSha256},
    storage::{
        disks::{Disk, PartitionTableType},
        encryption::{EncryptedVolume, Encryption},
        filesystem::{
            FileSystem, FileSystemSource, FileSystemType, MountOptions, MountPoint, MountPointInfo,
            VerityFileSystem,
        },
        internal::{InternalMountPoint, InternalVerityDevice},
        partitions::{AdoptedPartition, Partition, PartitionSize, PartitionType},
        raid::{Raid, RaidLevel, SoftwareRaidArray},
        Storage,
    },
    trident::Trident,
    HostConfiguration,
};

pub use local::{GrpcConfiguration, HostConfigurationSource, Operation, Operations};
