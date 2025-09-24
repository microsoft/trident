use std::fmt::Display;

use semver::Version;
use serde::{Deserialize, Serialize};

/// A thin wrapper around `semver::Version` to provide serialization and
/// deserialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppVersion(Version);

impl Display for AppVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Version> for AppVersion {
    fn from(version: Version) -> Self {
        Self(version)
    }
}

impl From<&Version> for AppVersion {
    fn from(version: &Version) -> Self {
        Self(version.clone())
    }
}

impl AppVersion {
    /// Returns a reference to the inner `Version`.
    pub(crate) fn as_version(&self) -> &Version {
        &self.0
    }

    #[allow(dead_code)]
    /// Returns a new version with the given major, minor, and patch.
    pub(crate) fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self(Version::new(major, minor, patch))
    }
}

impl Default for AppVersion {
    fn default() -> Self {
        Self(Version::new(0, 0, 0))
    }
}

/// Implement serialization for AppVersion
impl Serialize for AppVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// Implement deserialization for AppVersion
impl<'de> Deserialize<'de> for AppVersion {
    fn deserialize<D>(deserializer: D) -> Result<AppVersion, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(
            Version::parse(&String::deserialize(deserializer)?)
                .map_err(serde::de::Error::custom)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_app_version_serialization() {
        (0..100).for_each(|major| {
            (0..100).for_each(|minor| {
                (0..100).for_each(|patch| {
                    let version = AppVersion::from(Version::new(major, minor, patch));
                    let json = serde_json::to_string(&version).unwrap();
                    assert_eq!(json, format!("\"{major}.{minor}.{patch}\""));
                    let deserialized: AppVersion = serde_json::from_str(&json).unwrap();
                    assert_eq!(deserialized, version);
                });
            });
        });
    }

    #[test]
    fn test_app_version_serialization_invalid() {
        fn deserialize(v: &str) {
            serde_json::from_str::<AppVersion>(&format!("\"{v}\"")).unwrap_err();
        }

        // Incomplete semver
        deserialize("1.2");

        // Invalid semver
        deserialize("1.2.3.4");

        // Outright invalid stuff
        deserialize("aa");
    }
}
