use serde::{Deserialize, Serialize};

#[cfg(feature = "documentation")]
use documented::{Documented, DocumentedVariants};

use crate::BlockDeviceId;

/// Enum for reference kinds between a referrer and a block device in the graph.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
pub enum ReferenceKind {
    /// A regular reference that does not have any special meaning and inherits
    /// all properties from the referrer.
    Regular,

    /// A reference that holds a special meaning.
    Special(SpecialReferenceKind),
}

/// Enum for special reference kinds between a referrer and a block device in
/// the graph.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "documentation",
    derive(strum_macros::EnumIter, Documented, DocumentedVariants)
)]
pub enum SpecialReferenceKind {
    /// A reference to a Verity device's underlying data device.
    VerityDataDevice,

    /// A reference to a Verity device's underlying hash device.
    VerityHashDevice,
}

/// A reference to a block device in the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageReference<'a> {
    /// The kind of reference.
    kind: ReferenceKind,

    /// The ID of the block device being referenced.
    id: &'a BlockDeviceId,
}

impl<'a> StorageReference<'a> {
    /// Creates a new reference to a block device.
    pub fn new_regular(id: &'a BlockDeviceId) -> Self {
        Self {
            kind: ReferenceKind::Regular,
            id,
        }
    }

    /// Creates a new reference to a block device with a specific kind.
    pub fn new_special(kind: SpecialReferenceKind, id: &'a BlockDeviceId) -> Self {
        Self {
            kind: ReferenceKind::Special(kind),
            id,
        }
    }

    /// Returns the kind of reference.
    pub fn kind(&self) -> ReferenceKind {
        self.kind
    }

    /// Returns the ID of the block device being referenced.
    pub fn id(&self) -> &BlockDeviceId {
        self.id
    }
}

impl ReferenceKind {
    /// Returns ['None'] if the reference is regular, otherwise calls `f` with
    /// the special reference kind and returns the result, extending it with the
    /// special kind when `f` returns a value.
    pub fn is_special_then<T>(
        &self,
        f: impl FnOnce(SpecialReferenceKind) -> Option<T>,
    ) -> Option<(SpecialReferenceKind, T)> {
        match self {
            Self::Special(kind) => f(*kind).map(|result| (*kind, result)),
            Self::Regular => None,
        }
    }

    /// Returns `false` if the reference is regular, otherwise calls `f` with
    /// the special reference kind and returns the result.
    pub fn is_special_and(&self, f: impl FnOnce(SpecialReferenceKind) -> bool) -> bool {
        match self {
            Self::Special(kind) => f(*kind),
            Self::Regular => false,
        }
    }

    /// Returns whether the reference is regular.
    pub fn is_regular(&self) -> bool {
        matches!(self, Self::Regular)
    }

    /// Returns `true` if the reference is regular, otherwise calls `f` with the
    /// special reference kind and returns the result.
    pub fn is_regular_or(&self, f: impl FnOnce(SpecialReferenceKind) -> bool) -> bool {
        match self {
            Self::Regular => true,
            Self::Special(kind) => f(*kind),
        }
    }
}
