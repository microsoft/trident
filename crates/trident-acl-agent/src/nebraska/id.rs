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
        if is_fake_instance_shape(&id) {
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

/// Positions of the hyphens within a `8-4-4-4-12` id, and the total length of
/// that shape.
const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];
const HYPHENATED_LEN: usize = 36;

/// Returns whether the value has the shape Nebraska hides as a "fake instance".
///
/// The server filters with `instance_id NOT LIKE
/// '{________-____-____-____-____________}'`, and SQL's `_` matches *any*
/// character — so what is filtered is the braced `8-4-4-4-12` **shape**, not a
/// well-formed UUID. Matching on shape keeps this check aligned with the query
/// that actually hides the instance: a braced non-hex id of that shape would
/// still vanish from the UI, while a braced value of any other shape is a
/// legitimate id and must not be rejected.
fn is_fake_instance_shape(id: &str) -> bool {
    let Some(inner) = id.strip_prefix('{').and_then(|i| i.strip_suffix('}')) else {
        return false;
    };
    inner.len() == HYPHENATED_LEN
        && inner.char_indices().all(|(index, c)| {
            if HYPHEN_POSITIONS.contains(&index) {
                c == '-'
            } else {
                c != '-'
            }
        })
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
    fn new_accepts_braced_non_uuid() {
        // Nebraska hides only the braced 8-4-4-4-12 shape; a braced value of any
        // other shape is a legitimate id and must not be rejected.
        let id = MachineId::new("{not-a-uuid}").unwrap();
        assert_eq!(id.as_str(), "{not-a-uuid}");
    }

    #[test]
    fn new_rejects_braced_non_hex_of_uuid_shape() {
        // The server filters with a SQL LIKE whose `_` matches any character, so
        // a braced id of this shape is hidden even though it is not valid hex.
        let err = MachineId::new("{zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz}").unwrap_err();
        assert!(
            matches!(err, NebraskaError::InvalidRequest(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn new_accepts_braced_unhyphenated_uuid() {
        // Nebraska's pattern requires the hyphens, so the compact form is not
        // filtered and must not be rejected.
        let id = MachineId::new("{0123456789abcdef0123456789abcdef}").unwrap();
        assert_eq!(id.as_str(), "{0123456789abcdef0123456789abcdef}");
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
