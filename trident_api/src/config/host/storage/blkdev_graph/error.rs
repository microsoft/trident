use serde::{Deserialize, Serialize};

use crate::{
    config::{FileSystemType, PartitionType},
    BlockDeviceId,
};

use super::{
    cardinality::ValidCardinality,
    partitions::AllowedPartitionTypes,
    types::{
        BlkDevKind, BlkDevKindFlag, BlkDevReferrerKind, BlkDevReferrerKindFlag,
        FileSystemSourceKind, FileSystemSourceKindList,
    },
};

#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BlockDeviceGraphBuildError {
    #[error("Block device '{0}' is defined more than once")]
    DuplicateDeviceId(String),

    #[error("Block device '{node_id}' of kind '{kind}' is invalid: {body}")]
    BasicCheckFailed {
        node_id: String,
        kind: BlkDevKind,
        body: String,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references target \
            '{target_id}' more than once"
    )]
    DuplicateTargetId {
        node_id: String,
        kind: BlkDevKind,
        target_id: String,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' has {target_count} \
            target(s), but must have {expected} target(s)"
    )]
    InvalidTargetCount {
        node_id: String,
        kind: BlkDevKind,
        target_count: usize,
        expected: ValidCardinality,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references non-existent \
            block device '{target_id}'"
    )]
    NonExistentReference {
        node_id: String,
        kind: BlkDevKind,
        target_id: String,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references block device \
            '{target_id}' of invalid kind '{target_kind}', acceptable kinds \
            are: {valid_references}"
    )]
    InvalidReferenceKind {
        node_id: String,
        kind: BlkDevKind,
        target_id: String,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error("Mount point '{0}' is defined more than once")]
    DuplicateMountPoint(String),

    #[error("Mount point '{0}' should be an absolute")]
    MountPointPathNotAbsolute(String),

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

    #[error("Block device '{node_id}' of kind '{kind}' references invalid targets:\n{body}")]
    InvalidTargets {
        node_id: String,
        kind: BlkDevKind,
        body: String,
    },

    #[error("Internal error: {body}")]
    InternalError { body: String },

    #[error(
        "Referrers '{referrer_a_id}' (of kind '{referrer_a_kind}') and '{referrer_b_id}' \
            (of kind '{referrer_b_kind}') cannot share block device '{target_id}' of kind \
            '{target_kind}'. Referrers of kind '{referrer_a_kind}' can only share with: \
            {referrer_a_valid_sharing_peers}. Referrers of kind '{referrer_b_kind}' can \
            only share with: {referrer_b_valid_sharing_peers}"
    )]
    ReferrerForbiddenSharing {
        target_id: String,
        target_kind: BlkDevKind,
        referrer_a_id: String,
        referrer_a_kind: BlkDevReferrerKind,
        referrer_b_id: String,
        referrer_b_kind: BlkDevReferrerKind,
        referrer_a_valid_sharing_peers: BlkDevReferrerKindFlag,
        referrer_b_valid_sharing_peers: BlkDevReferrerKindFlag,
    },

    #[error("Filesystem of type '{0}' requires a reference to a block device")]
    FilesystemMissingBlockDeviceId(FileSystemType),

    #[error("Filesystem of type '{0}' should not reference a block device")]
    FilesystemUnexpectedBlockDeviceId(FileSystemType),

    #[error("Filesystem of type '{0}' should not have a mount point")]
    FilesystemUnexpectedMountPoint(FileSystemType),

    #[error(
        "Filesystem [{fs_desc}] references non-existent block device \
            '{target_id}'"
    )]
    FilesystemNonExistentReference {
        target_id: BlockDeviceId,
        fs_desc: String,
    },

    #[error(
        "Filesystem [{fs_desc}] has invalid source type '{fs_source}', \
            acceptable sources are: {fs_acceptable_sources}"
    )]
    FilesystemInvalidSource {
        fs_desc: String,
        fs_source: FileSystemSourceKind,
        fs_acceptable_sources: FileSystemSourceKindList,
    },

    #[error(
        "Filesystem [{fs_desc}] references block device '{target_id}' \
            of invalid kind '{target_kind}', acceptable kinds are: {valid_references}"
    )]
    FilesystemInvalidReference {
        fs_desc: String,
        target_id: BlockDeviceId,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error(
        "Filesystem [{fs_desc}] references block device '{target_id}' \
            that is already associated with a filesystem [{other_fs_desc}]"
    )]
    FilesystemReferenceInUse {
        fs_desc: String,
        target_id: BlockDeviceId,
        other_fs_desc: String,
    },

    #[error(
        "Filesystem [{fs_desc}] references block device '{target_id}' \
            that is already associated with verity filesystem '{other_vfs_name}' \
            of type '{other_vfs_type}'"
    )]
    FilesystemReferenceInUseVerity {
        fs_desc: String,
        target_id: BlockDeviceId,
        other_vfs_name: String,
        other_vfs_type: FileSystemType,
    },

    #[error(
        "Verity filesystem '{name}' is using an unsupported filesystem type \
            '{fs_type}'"
    )]
    VerityFileSystemUnsupportedType {
        name: String,
        fs_type: FileSystemType,
    },

    #[error(
        "Verity filesystem '{name}' of type '{fs_type}' references non-existent \
            block device '{target_id}' as '{role}'"
    )]
    VerityFilesystemNonExistentReference {
        name: String,
        target_id: BlockDeviceId,
        fs_type: FileSystemType,
        role: String,
    },

    #[error("Verity filesystem name '{name}' is used more than once")]
    VerityDuplicateName { name: String },

    #[error(
        "Verity filesystem '{name}' of type '{fs_type}' references block device \
            '{target_id}' of invalid kind '{target_kind}' as data block, acceptable \
            kinds are: {valid_references}"
    )]
    VerityFilesystemInvalidReferenceData {
        name: String,
        fs_type: FileSystemType,
        target_id: BlockDeviceId,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error(
        "Verity filesystem '{name}' of type '{fs_type}' references block device \
            '{target_id}' of invalid kind '{target_kind}' as hash block, acceptable \
            kinds are: {valid_references}"
    )]
    VerityFilesystemInvalidReferenceHash {
        name: String,
        fs_type: FileSystemType,
        target_id: BlockDeviceId,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references partition 
            '{partition_id}' of non-fixed size"
    )]
    PartitionSizeNotFixed {
        node_id: BlockDeviceId,
        kind: BlkDevKind,
        partition_id: BlockDeviceId,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references partitions of \
            different sizes"
    )]
    PartitionSizeMismatch {
        node_id: BlockDeviceId,
        kind: BlkDevKind,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references partition \
            of different types"
    )]
    PartitionTypeMismatch {
        node_id: BlockDeviceId,
        kind: BlkDevKind,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references partition \
            '{partition_id}' of invalid type '{partition_type}', acceptable \
            types are: {valid_types}"
    )]
    InvalidPartitionType {
        node_id: BlockDeviceId,
        kind: BlkDevKind,
        partition_id: BlockDeviceId,
        partition_type: PartitionType,
        valid_types: AllowedPartitionTypes,
    },

    #[error(
        "Referrer '{referrer}' [{fs_desc}] references partition(s) \
            of invalid type '{partition_type}', acceptable \
            types are: {valid_types}"
    )]
    FilesystemInvalidPartitionType {
        referrer: BlkDevReferrerKind,
        fs_desc: String,
        partition_type: PartitionType,
        valid_types: AllowedPartitionTypes,
    },

    #[error(
        "Referrer '{referrer}' [{fs_desc}] references partitions of \
            different types"
    )]
    FilesystemHeterogenousPartitionTypes {
        referrer: BlkDevReferrerKind,
        fs_desc: String,
    },

    #[error(
        "Verity filesystem '{name}' of type '{fs_type}' references partition(s) \
            of invalid type '{partition_type}' because it does not have a hash partition \
            type counterpart"
    )]
    VerityFilesystemInvalidaDataPartitionType {
        name: String,
        fs_type: FileSystemType,
        partition_type: PartitionType,
    },

    #[error(
        "Verity filesystem '{name}' of type '{fs_type}' references data \
            partition(s) of type '{data_part_type}', hash partition(s) \
            is/are expected to be of type '{expected_type}', but \
            found '{actual_type}'"
    )]
    VerityFilesystemPartitionTypeMismatch {
        name: String,
        fs_type: FileSystemType,
        data_part_type: PartitionType,
        expected_type: PartitionType,
        actual_type: PartitionType,
    },

    #[error("Referrer '{referrer}' of kind '{kind}' references block devices of different kinds")]
    ReferenceKindMismatch {
        referrer: String,
        kind: BlkDevReferrerKind,
    },
}
