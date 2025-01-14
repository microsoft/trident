use serde::{Deserialize, Serialize};

use crate::{
    config::{FileSystemType, PartitionType, RaidLevel},
    BlockDeviceId,
};

use super::{
    cardinality::ValidCardinality,
    containers::{AllowBlockList, ItemList},
    node::NodeIdentifier,
    references::SpecialReferenceKind,
    types::{
        BlkDevKind, BlkDevKindFlag, BlkDevReferrerKind, BlkDevReferrerKindFlag,
        FileSystemSourceKind,
    },
};

/// Renders a node identifier in to a pretty string suitable for displaying an
/// error message.
fn pretty_node_id(node_identifier: &NodeIdentifier) -> String {
    match node_identifier {
        NodeIdentifier::BlockDevice(id) => format!("'{}'", id),
        NodeIdentifier::VerityFileSystem(id) => {
            format!("verity fs '{}'", id)
        }
        NodeIdentifier::FileSystem(fs) => format!("filesystem [{}]", fs),
    }
}

/// Pluralizes a word based on the count.
fn pluralize(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{} {}", count, singular)
    } else {
        format!("{} {}", count, plural)
    }
}

#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StorageGraphBuildError {
    #[error("Block device '{node_id}' of kind '{kind}' is invalid: {body}")]
    BasicCheckFailed {
        node_id: String,
        kind: BlkDevKind,
        body: String,
    },

    #[error("Block device '{0}' is defined more than once")]
    DuplicateDeviceId(String),

    #[error("Mount point '{0}' is defined more than once")]
    DuplicateMountPoint(String),

    #[error(
        "Referrer {} of kind '{kind}' references target \
            '{target_id}' more than once", pretty_node_id(.node_identifier)
    )]
    DuplicateTargetId {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        target_id: BlockDeviceId,
    },

    #[error(
        "Filesystem [{fs_desc}] has incompatible source type '{fs_source}', \
            compatible sources are: {fs_compatible_sources}"
    )]
    FilesystemIncompatibleSource {
        fs_desc: String,
        fs_source: FileSystemSourceKind,
        fs_compatible_sources: ItemList<FileSystemSourceKind>,
    },

    #[error("Filesystem [{fs_desc}] requires a reference to a block device")]
    FilesystemMissingBlockDeviceId { fs_desc: String },

    #[error("Filesystem [{fs_desc}] requires a mount point")]
    FilesystemMissingMountPoint { fs_desc: String },

    #[error("Filesystem [{fs_desc}] should not reference a block device")]
    FilesystemUnexpectedBlockDeviceId { fs_desc: String },

    #[error("Filesystem [{fs_desc}] of type '{fs_type}' must not have a mount point")]
    FilesystemUnexpectedMountPoint {
        fs_desc: String,
        fs_type: FileSystemType,
    },

    #[error("Internal error: {body}")]
    InternalError { body: String },

    #[error(
        "Referrer {} of kind '{kind}' references partition \
            '{partition_id}' of invalid type '{partition_type}', acceptable \
            types are: {valid_types}",
        pretty_node_id(.node_identifier)
    )]
    InvalidPartitionType {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        partition_id: BlockDeviceId,
        partition_type: PartitionType,
        valid_types: AllowBlockList<PartitionType>,
    },

    #[error(
        "Referrer {} of kind '{kind}' has a special reference of kind '{special_ref_kind}' which \
            references partition '{partition_id}' of invalid type '{partition_type}', acceptable \
            types are: {valid_types}",
        pretty_node_id(.node_identifier)
    )]
    InvalidPartitionTypeSpecial {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        partition_id: BlockDeviceId,
        partition_type: PartitionType,
        valid_types: AllowBlockList<PartitionType>,
        special_ref_kind: SpecialReferenceKind,
    },

    #[error(
        "Referrer {} of kind '{kind}' references RAID array '{raid_id}' of invalid level \
            '{raid_level}', acceptable levels are: {valid_levels}",
            pretty_node_id(.node_identifier)
    )]
    InvalidRaidlevel {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        raid_id: BlockDeviceId,
        raid_level: RaidLevel,
        valid_levels: AllowBlockList<RaidLevel>,
    },

    #[error(
        "Referrer {} of kind '{kind}' references block device \
            '{target_id}' of invalid kind '{target_kind}', acceptable kinds \
            are: {valid_references}",
        pretty_node_id(.node_identifier)
    )]
    InvalidReferenceKind {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        target_id: String,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error(
        "Referrer {} of kind '{kind}' references block device \
            '{target_id}' of invalid kind '{target_kind}' as '{reference_kind}', \
            acceptable kinds are: {valid_references}",
        pretty_node_id(.node_identifier)
    )]
    InvalidSpecialReferenceKind {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        target_id: String,
        target_kind: BlkDevKind,
        reference_kind: SpecialReferenceKind,
        valid_references: BlkDevKindFlag,
    },

    #[error(
        "Referrer {} of kind '{kind}' has {}, but must have {expected} {}",
        pretty_node_id(.node_identifier),
        pluralize(*.target_count, "target", "targets"),
        .expected.pluralize("target", "targets")
    )]
    InvalidTargetCount {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        target_count: usize,
        expected: ValidCardinality,
    },

    #[error("Referrer {} of kind '{kind}' references invalid targets:\n{body}",
        pretty_node_id(.node_identifier)
    )]
    InvalidTargets {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        body: String,
    },

    #[error("Mount point location '{0}' is not an absolute path")]
    MountPointPathNotAbsolute(String),

    #[error(
        "Referrer {} of kind '{kind}' references non-existent \
            block device '{target_id}'",
        pretty_node_id(.node_identifier)
    )]
    NonExistentReference {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        target_id: String,
    },

    #[error(
        "Referrer {} of kind '{kind}' references partitions of \
            different sizes",
            pretty_node_id(.node_identifier)
    )]
    PartitionSizeMismatch {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
    },

    #[error(
        "Referrer {} of kind '{kind}' references partition
            '{partition_id}' of non-fixed size",
        pretty_node_id(.node_identifier)
    )]
    PartitionSizeNotFixed {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        partition_id: BlockDeviceId,
    },

    #[error(
        "Referrer {} of kind '{kind}' references partitions \
            of different types",
        pretty_node_id(.node_identifier)
    )]
    PartitionTypeMismatch {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
    },

    #[error(
        "Referrer {} of kind '{kind}' has a special reference of kind '{special_ref_kind}' which references \
        partitions of different types.",
        pretty_node_id(.node_identifier)
    )]
    PartitionTypeMismatchSpecial {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
        special_ref_kind: SpecialReferenceKind,
    },

    #[error(
        "Referrer {} of kind '{kind}' references block devices of different kinds", 
        pretty_node_id(.node_identifier)
    )]
    ReferenceKindMismatch {
        node_identifier: NodeIdentifier,
        kind: BlkDevReferrerKind,
    },

    #[error(
        "Referrers {} (of kind '{referrer_a_kind}') and {} (of kind '{referrer_b_kind}') \
            cannot reference block device '{target_id}' of kind '{target_kind}' at the same time. \
            Referrers of kind '{referrer_a_kind}' can only share with: \
            {referrer_a_valid_sharing_peers}. Referrers of kind '{referrer_b_kind}' can only share \
            with: {referrer_b_valid_sharing_peers}",
        pretty_node_id(.referrer_a_id),
        pretty_node_id(.referrer_b_id)
    )]
    ReferrerForbiddenSharing {
        target_id: String,
        target_kind: BlkDevKind,
        referrer_a_id: NodeIdentifier,
        referrer_a_kind: BlkDevReferrerKind,
        referrer_b_id: NodeIdentifier,
        referrer_b_kind: BlkDevReferrerKind,
        referrer_a_valid_sharing_peers: BlkDevReferrerKindFlag,
        referrer_b_valid_sharing_peers: BlkDevReferrerKindFlag,
    },

    #[error(
        "Block device '{node_id}' and '{other_id}' of kind '{kind}' have a \
            duplicate value for field '{field_name}' ('{value}')"
    )]
    UniqueFieldConstraintError {
        node_id: String,
        other_id: String,
        kind: BlkDevKind,
        field_name: String,
        value: String,
    },

    #[error("Verity filesystem name '{name}' is used more than once")]
    VerityFilesystemDuplicateName { name: String },

    #[error(
        "Verity filesystem '{name}' is using an unsupported filesystem type \
            '{fs_type}'"
    )]
    VerityFileSystemUnsupportedType {
        name: String,
        fs_type: FileSystemType,
    },
}
