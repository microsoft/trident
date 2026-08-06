//! The [`MachineId`] newtype: a Nebraska instance identity.

use std::fmt::{self, Display, Formatter};

use uuid::Uuid;

use super::error::NebraskaError;

/// A Nebraska instance identifier.
///
/// `machineid` is the **primary key** of an instance in Nebraska, so it carries
/// two invariants that fail *silently* when violated, which is why this is a
/// validated newtype rather than a bare `String`:
///
/// 1. **It must not be brace-formatted.** Nebraska filters instance ids matching
///    `{8-4-4-4-12}` out of both the instance list and the group statistics as
///    "fake instances". A client using a braced id is invisible in the UI while
///    appearing to work perfectly over the wire. [`MachineId::new`] rejects such
///    values; [`MachineId::from_uuid`] relies on Rust's [`Uuid`] `Display`,
///    which is hyphenated and unbraced.
///
/// 2. **It must be stable across the update reboot.** If it changes, the old
///    instance is left behind in whatever state it was in — permanently wedged
///    if intermediate events had been sent — and a new instance appears with no
///    history. Stability is the caller's responsibility (e.g. deriving it from a
///    machine id that lives on a partition that survives the A/B swap); this
///    type only guarantees the format.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MachineId(String);

impl MachineId {
    /// Builds a [`MachineId`] from an arbitrary string, rejecting values that
    /// would be filtered out by Nebraska.
    ///
    /// # Errors
    ///
    /// Returns [`NebraskaError::InvalidRequest`] if the value is empty or is a
    /// brace-wrapped UUID (`{...}`), which Nebraska treats as a fake instance.
    pub fn new(id: impl Into<String>) -> Result<Self, NebraskaError> {
        let id = id.into();
        if id.is_empty() {
            return Err(NebraskaError::InvalidRequest(
                "machine id must not be empty".to_string(),
            ));
        }
        if is_braced(&id) {
            return Err(NebraskaError::InvalidRequest(format!(
                "machine id '{id}' is brace-wrapped; Nebraska filters braced ids out of the UI \
                 and group statistics"
            )));
        }
        Ok(Self(id))
    }

    /// Builds a [`MachineId`] from a [`Uuid`], using its hyphenated, unbraced
    /// representation. This is always valid.
    pub fn from_uuid(uuid: Uuid) -> Self {
        // `Uuid`'s `Display` is hyphenated and unbraced, which is exactly what
        // Nebraska expects; construct directly to skip the (impossible) failure.
        Self(uuid.to_string())
    }

    /// Returns the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for MachineId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Returns whether the value is a brace-wrapped id (`{...}`), the shape Nebraska
/// filters out.
fn is_braced(id: &str) -> bool {
    id.starts_with('{') && id.ends_with('}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_uuid_is_unbraced() {
        let uuid = Uuid::parse_str("12345678-1234-4234-8234-1234567890ab").unwrap();
        let id = MachineId::from_uuid(uuid);
        assert_eq!(id.as_str(), "12345678-1234-4234-8234-1234567890ab");
        assert!(!id.as_str().starts_with('{'));
    }

    #[test]
    fn new_accepts_plain_id() {
        let id = MachineId::new("12345678-1234-4234-8234-1234567890ab").unwrap();
        assert_eq!(id.as_str(), "12345678-1234-4234-8234-1234567890ab");
    }

    #[test]
    fn new_rejects_braced_uuid() {
        let err = MachineId::new("{12345678-1234-4234-8234-1234567890ab}").unwrap_err();
        assert!(
            matches!(err, NebraskaError::InvalidRequest(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn new_rejects_empty() {
        let err = MachineId::new("").unwrap_err();
        assert!(
            matches!(err, NebraskaError::InvalidRequest(_)),
            "got {err:?}"
        );
    }
}
