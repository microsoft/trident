use std::fmt::Display;

use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

/// This enum contains a proper UUID or a relaxed string representation of
/// something that is not a proper UUID.
///
/// This is useful for block device metadata parsing, because they may have
/// proper UUIDs or just some string that uniquely identifies it, but does not
/// conform to the format of an UUID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OsUuid {
    Uuid(Uuid),
    Relaxed(String),
}

impl OsUuid {
    /// Checks if the given UUID matches the one stored in this enum.
    pub fn match_uuid(&self, other: &Uuid) -> bool {
        match self {
            OsUuid::Uuid(uuid) => uuid == other,
            OsUuid::Relaxed(_) => false,
        }
    }

    /// Provides the UUID stored in this enum, if it is a proper UUID.
    pub fn as_uuid(&self) -> Option<Uuid> {
        match self {
            OsUuid::Uuid(uuid) => Some(*uuid),
            OsUuid::Relaxed(_) => None,
        }
    }
}

impl From<&str> for OsUuid {
    fn from(value: &str) -> Self {
        match Uuid::parse_str(value) {
            Ok(uuid) => Self::Uuid(uuid),
            Err(_) => Self::Relaxed(value.to_string()),
        }
    }
}

impl From<String> for OsUuid {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

impl From<Uuid> for OsUuid {
    fn from(value: Uuid) -> Self {
        Self::Uuid(value)
    }
}

impl Display for OsUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OsUuid::Uuid(uuid) => write!(f, "{}", uuid.hyphenated()),
            OsUuid::Relaxed(s) => write!(f, "{s}"),
        }
    }
}

impl<'de> Deserialize<'de> for OsUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(String::deserialize(deserializer)?.as_str().into())
    }
}

impl Serialize for OsUuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osuuid_roundtrip() {
        let uuid_test_cases = [
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-0000-0000-000000000001",
            "a0a0a0a0-a0a0-a0a0-a0a0-a0a0a0a0a0a0",
            "fedcba98-7654-3210-fedc-ba9876543210",
            "fedcba98-7654-3210-fedc-ba9876543211",
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            "019074de-9854-7210-a451-19d1b903fff8",
        ];

        let relaxed_test_cases = [
            "3a9c2054-02",
            "some other string",
            "random string",
            "019074de-9854-7210-a451-19d1b903fff",
            "019074de-9854-7210-a451-19d1b903fff8a",
            "019074de-9854-7210-a451-19d1b903fff8a1",
            "X0000000-0000-0000-0000-000000000000",
        ];

        for uuid in uuid_test_cases.iter() {
            let parsed = OsUuid::from(*uuid);
            let expected = Uuid::parse_str(uuid).unwrap();
            assert_eq!(parsed, OsUuid::Uuid(expected));

            assert_eq!(parsed.to_string().as_str(), *uuid);
        }

        for relaxed in relaxed_test_cases.iter() {
            let parsed = OsUuid::from(*relaxed);
            assert_eq!(parsed, OsUuid::Relaxed(relaxed.to_string()));

            assert_eq!(parsed.to_string().as_str(), *relaxed);
        }
    }
}
