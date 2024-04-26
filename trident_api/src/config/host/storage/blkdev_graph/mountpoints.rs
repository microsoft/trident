//! Structs for representing mountpoint rules.

use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

pub(super) enum ValidMountpoints {
    None,
    Any,
    Specific(Vec<PathBuf>),
}

impl ValidMountpoints {
    pub fn new(paths: &[impl AsRef<Path>]) -> Self {
        if paths.is_empty() {
            return Self::None;
        }

        Self::Specific(paths.iter().map(|p| p.as_ref().to_path_buf()).collect())
    }

    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        match self {
            Self::None => false,
            Self::Any => true,
            Self::Specific(paths) => paths.iter().any(|p| p == path.as_ref()),
        }
    }
}

impl Display for ValidMountpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidMountpoints::None => write!(f, "(mounting is not valid)"),
            ValidMountpoints::Any => write!(f, "any location"),
            ValidMountpoints::Specific(list) => {
                write!(
                    f,
                    "{}",
                    list.iter()
                        .map(|path| path.to_string_lossy().to_string())
                        .collect::<Vec<String>>()
                        .join(" or ")
                )
            }
        }
    }
}
