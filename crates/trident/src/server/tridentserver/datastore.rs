use serde_yaml::Value;

use harpoon::{
    FileLocation, TridentError as HarpoonTridentError, TridentErrorKind as HarpoonTridentErrorKind,
};
use trident_api::error::{ErrorCategory, TridentError};

use crate::DataStore;

const UNKNOWN_VALUE: &str = "unknown";

/// Extracts a HarpoonTridentError from the given DataStore's last error, if any.
pub(super) fn error_from_datastore(datastore: &DataStore) -> Option<HarpoonTridentError> {
    datastore
        .host_status()
        .last_error
        .as_ref()
        .map(harpoon_trident_error_from_value)
}

fn harpoon_trident_error_from_value(value: &Value) -> HarpoonTridentError {
    let Some(root) = value.as_mapping() else {
        // There is an error but we can't parse it, return a generic unknown error.
        return HarpoonTridentError {
            kind: HarpoonTridentErrorKind::Unspecified.into(),
            subkind: UNKNOWN_VALUE.to_string(),
            full_body: UNKNOWN_VALUE.to_string(),
            message: UNKNOWN_VALUE.to_string(),
            location: None,
        };
    };

    // Helper to simplify yaml string values from &strs.
    let s = |k: &str| Value::String(k.to_string());

    let kind = root
        .get(s(TridentError::SERIALIZE_FIELD_CATEGORY))
        .and_then(|v| v.as_str())
        .and_then(|s| ErrorCategory::try_from(s).ok())
        .map(HarpoonTridentErrorKind::from)
        .unwrap_or(HarpoonTridentErrorKind::Unspecified);

    // Extract the subkind, which is encoded in the tag of the error value.
    let subkind = root
        .get(s(TridentError::SERIALIZE_FIELD_ERROR))
        .and_then(|v| {
            if let Value::Tagged(t) = v {
                Some(&t.tag)
            } else {
                None
            }
        })
        .map(|t| {
            let raw = t.to_string();
            // Remove leading "!" if present.
            match raw.strip_prefix("!") {
                Some(s) => s.to_string(),
                None => raw,
            }
        })
        .unwrap_or_else(|| UNKNOWN_VALUE.to_string())
        .to_string();

    let message = root
        .get(s(TridentError::SERIALIZE_FIELD_MESSAGE))
        .and_then(|v| v.as_str())
        .unwrap_or(UNKNOWN_VALUE)
        .to_string();

    // Extract the location field, if any.
    let location = 'location_block: {
        // If no location string, break with None.
        let Some(locstr) = root
            .get(s(TridentError::SERIALIZE_FIELD_LOCATION))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        else {
            break 'location_block None;
        };

        // We expect location strings of the form "path:line".
        let mut tokens = locstr.split(":");

        // Try to extract path
        let path = if let Some(path) = tokens.next() {
            path.to_string()
        } else {
            break 'location_block None;
        };

        // Try to extract line number, default to 0 if not present or invalid.
        let line = tokens
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        Some(FileLocation { path, line })
    };

    let full_body = root
        .get(s(TridentError::SERIALIZE_FIELD_CAUSE))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    HarpoonTridentError {
        kind: kind.into(),
        subkind,
        full_body,
        message,
        location,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::anyhow;

    use trident_api::error::{InternalError, TridentError};

    #[test]
    fn test_harpoon_trident_error_from_value() {
        let panic_body = "some foo";
        let err = TridentError::with_source(
            InternalError::Panic(panic_body.to_string()),
            anyhow!("some error").context("extra context"),
        );
        let val = serde_yaml::to_value(&err).expect("serialize failed");
        let converted = harpoon_trident_error_from_value(&val);

        println!("Serialized error:\n{:#?}", val);
        println!("Converted error:\n{:#?}", converted);

        assert_eq!(converted.kind(), HarpoonTridentErrorKind::Internal);
        assert_eq!(
            converted.message,
            InternalError::Panic(panic_body.to_string()).to_string()
        );
        let loc = converted.location.expect("expected location");
        assert!(loc.path.ends_with("datastore.rs"));
        assert!(loc.line > 0);
        assert!(converted.full_body.contains("some error"));
    }
}
