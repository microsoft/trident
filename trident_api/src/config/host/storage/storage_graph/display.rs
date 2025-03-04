//! Display implementations for the types in the storage_graph module.

use std::{fmt::Display, ops::Deref};

use super::{
    containers::{AllowBlockList, ItemList, PathAllowBlockList},
    references::{ReferenceKind, SpecialReferenceKind},
    types::{
        BitFlagsBackingEnumVec, BlkDevKind, BlkDevKindFlag, BlkDevReferrerKind,
        BlkDevReferrerKindFlag, FileSystemSourceKind,
    },
};

impl Display for FileSystemSourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New => write!(f, "new"),
            Self::Image => write!(f, "image"),
            Self::Adopted => write!(f, "adopted"),
            Self::EspBundle => write!(f, "esp-image"),
            Self::OsImage => write!(f, "os-image"),
        }
    }
}

impl Display for BlkDevKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Disk => write!(f, "disk"),
            Self::Partition => write!(f, "partition"),
            Self::AdoptedPartition => write!(f, "adopted-partition"),
            Self::RaidArray => write!(f, "raid-array"),
            Self::ABVolume => write!(f, "ab-volume"),
            Self::EncryptedVolume => write!(f, "encrypted-volume"),
            Self::VerityDevice => write!(f, "verity-device"),
        }
    }
}

impl Display for BlkDevReferrerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::RaidArray => write!(f, "raid-array"),
            Self::ABVolume => write!(f, "ab-volume"),
            Self::EncryptedVolume => write!(f, "encrypted-volume"),
            Self::VerityDevice => write!(f, "verity-device"),
            Self::FileSystem => write!(f, "filesystem"),
            Self::FileSystemEsp => write!(f, "filesystem-esp"),
            Self::FileSystemAdopted => write!(f, "filesystem-adopted"),
            Self::FileSystemOsImage => write!(f, "filesystem-os-image"),
        }
    }
}

impl Display for ReferenceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Regular => write!(f, "regular"),
            Self::Special(kind) => write!(f, "special ({})", kind),
        }
    }
}

impl Display for SpecialReferenceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VerityDataDevice => write!(f, "verity-data-device"),
            Self::VerityHashDevice => write!(f, "verity-hash-device"),
        }
    }
}

impl<T> Display for ItemList<T>
where
    T: Copy + Clone + PartialEq + Eq + Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<String>>()
                .join(" or ")
        )
    }
}

impl<T: Display> Display for AllowBlockList<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Any => write!(f, "any"),
            Self::Allow(types) => {
                write!(
                    f,
                    "{}",
                    types
                        .iter()
                        .map(|t| format!("'{t}'"))
                        .collect::<Vec<String>>()
                        .join(" or ")
                )
            }
            Self::Block(types) => {
                write!(
                    f,
                    "any type except {}",
                    types
                        .iter()
                        .map(|t| format!("'{t}'"))
                        .collect::<Vec<String>>()
                        .join(" or ")
                )
            }
        }
    }
}

impl Display for PathAllowBlockList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Transform AllowBlockList<std::path::PathBuf> into AllowBlockList<std::path::Display> for display.
        match self.deref() {
            AllowBlockList::None => AllowBlockList::None,
            AllowBlockList::Any => AllowBlockList::Any,
            AllowBlockList::Allow(paths) => {
                AllowBlockList::Allow(paths.iter().map(|p| p.display()).collect::<Vec<_>>())
            }
            AllowBlockList::Block(paths) => {
                AllowBlockList::Block(paths.iter().map(|p| p.display()).collect::<Vec<_>>())
            }
        }
        .fmt(f)
    }
}

impl Display for BlkDevKindFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_readable())
    }
}

impl Display for BlkDevReferrerKindFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_readable())
    }
}
