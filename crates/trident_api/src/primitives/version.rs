use std::fmt::Display;

use semver::Version;
use serde::{Deserialize, Serialize};

/// A thin wrapper around `semver::Version` to provide serialization and
/// deserialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemverVersion(Version);

impl Display for SemverVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Version> for SemverVersion {
    fn from(version: Version) -> Self {
        Self(version)
    }
}

impl From<&Version> for SemverVersion {
    fn from(version: &Version) -> Self {
        Self(version.clone())
    }
}

impl SemverVersion {
    /// Create a new `AppVersion` from the given major, minor, and patch.
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self(Version::new(major, minor, patch))
    }

    /// Parse a `AppVersion` from a string.
    pub fn parse(version: &str) -> Result<Self, semver::Error> {
        Version::parse(version).map(Self)
    }

    /// Gets a reference to the inner `Version`.
    pub fn as_version(&self) -> &Version {
        &self.0
    }
}

/// Implement serialization for AppVersion
impl Serialize for SemverVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// Implement deserialization for AppVersion
impl<'de> Deserialize<'de> for SemverVersion {
    fn deserialize<D>(deserializer: D) -> Result<SemverVersion, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(
            Version::parse(&String::deserialize(deserializer)?)
                .map_err(serde::de::Error::custom)?,
        ))
    }
}
