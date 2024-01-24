use serde::{Deserialize, Serialize};

use super::{
    cardinality::ValidCardinality,
    types::{BlkDevKind, BlkDevKindFlag, BlkDevReferrerKind, BlkDevReferrerKindFlag},
};

#[derive(thiserror::Error, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BlockDeviceGraphBuildError {
    #[error("Block device '{0}' is defined more than once")]
    DuplicateDeviceId(String),

    #[error("Block device '{node_id}' of kind '{kind}' is invalid")]
    BasicCheckFailed {
        node_id: String,
        kind: BlkDevKind,
        body: String,
    },

    #[error(
        "Block device '{node_id}' of kind '{kind}' references target '{target_id}' more than once"
    )]
    DuplicateTargetId {
        node_id: String,
        kind: BlkDevKind,
        target_id: String,
    },

    #[error("Block device '{node_id}' of kind '{kind}' has {target_count} target(s), but must have {expected} target(s)")]
    InvalidTargetCount {
        node_id: String,
        kind: BlkDevKind,
        target_count: usize,
        expected: ValidCardinality,
    },

    #[error("Block device '{node_id}' of kind '{kind}' references non-existent block device '{target_id}'")]
    NonExistentReference {
        node_id: String,
        kind: BlkDevKind,
        target_id: String,
    },

    #[error("Block device '{node_id}' of kind '{kind}' references block device '{target_id}' of invalid kind '{target_kind}', acceptable kinds are: {valid_references}")]
    InvalidReferenceKind {
        node_id: String,
        kind: BlkDevKind,
        target_id: String,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error("Image '{image_id}' references non-existent block device '{target_id}'")]
    ImageNonExistentReference { image_id: String, target_id: String },

    #[error("Image '{image_id}' references block device '{target_id}' of invalid kind '{target_kind}', acceptable kinds are: {valid_references}")]
    ImageInvalidReference {
        image_id: String,
        target_id: String,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error("Image '{image_id}' references block device '{target_id}' that is already being imaged with '{other_image_id}'")]
    ImageReferenceAlreadyImaging {
        image_id: String,
        target_id: String,
        other_image_id: String,
    },

    #[error("Mount point '{0}' is defined more than once")]
    DuplicateMountPoint(String),

    #[error("Mount point '{mount_point}' references non-existent block device '{target_id}'")]
    MountPointNonExistentReference {
        mount_point: String,
        target_id: String,
    },

    #[error("Mount point '{mount_point}' references block device '{target_id}' of invalid kind '{target_kind}', acceptable kinds are: {valid_references}")]
    MountPointInvalidReference {
        mount_point: String,
        target_id: String,
        target_kind: BlkDevKind,
        valid_references: BlkDevKindFlag,
    },

    #[error(
        "Mount point '{mount_point}' should be an absolute path or one of '{valid_mount_points}'"
    )]
    InvalidMountPoint {
        mount_point: String,
        valid_mount_points: String,
    },

    #[error(
        "Block device '{node_id}' and '{other_id}' of kind '{kind}' have a duplicate value for field '{field_name}' ('{value}')"
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

    #[error("Internal error")]
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
}
