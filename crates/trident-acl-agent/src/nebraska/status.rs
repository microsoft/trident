//! Response status types, modelled so that unknown values never break parsing.
//!
//! Two Nebraska behaviours drive the design here:
//!
//! - `error-updateInProgressOnInstance` is returned on **every** update check
//!   between the first progress event and the terminal event. It is expected,
//!   not a fault, and must be handled distinctly.
//! - Nebraska may return status strings a given client does not know about. A
//!   status enum without a catch-all would turn a normal response into a hard
//!   parse failure, so every status type here has an `Other` variant.

use std::fmt::{self, Display, Formatter};

use serde::Deserialize;

/// Status of an `<app>` element in a Nebraska response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum AppStatus {
    /// The app resolved successfully.
    #[serde(rename = "ok")]
    Ok,

    /// An update is already in progress for this instance. Returned on every
    /// poll between the first progress event and the terminal event; expected,
    /// not fatal.
    #[serde(rename = "error-updateInProgressOnInstance")]
    UpdateInProgress,

    /// Any other (including unknown) status string, preserved verbatim so that
    /// an unrecognised value can never cause a parse failure.
    #[serde(untagged)]
    Other(String),
}

impl AppStatus {
    /// Whether this status represents success (`ok`).
    pub fn is_ok(&self) -> bool {
        matches!(self, AppStatus::Ok)
    }

    /// Whether this is the expected "update already in progress" status, which a
    /// correct client tolerates rather than treating as an error.
    pub fn is_update_in_progress(&self) -> bool {
        matches!(self, AppStatus::UpdateInProgress)
    }
}

impl Display for AppStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AppStatus::Ok => f.write_str("ok"),
            AppStatus::UpdateInProgress => f.write_str("error-updateInProgressOnInstance"),
            AppStatus::Other(s) => f.write_str(s),
        }
    }
}

/// Status of an `<updatecheck>` element in a Nebraska response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum UpdateCheckStatus {
    /// An update is available.
    #[serde(rename = "ok")]
    Ok,

    /// No update is available.
    #[serde(rename = "noupdate")]
    NoUpdate,

    /// An internal server error. In normal operation this accompanies the
    /// app-level `error-updateInProgressOnInstance` and is therefore expected
    /// during an in-flight update.
    #[serde(rename = "error-internal")]
    ErrorInternal,

    /// Any other (including unknown) status string, preserved verbatim.
    #[serde(untagged)]
    Other(String),
}

impl UpdateCheckStatus {
    /// Whether an update is available (`ok`).
    pub fn is_update_available(&self) -> bool {
        matches!(self, UpdateCheckStatus::Ok)
    }

    /// Whether the check reported no update (`noupdate`).
    pub fn is_no_update(&self) -> bool {
        matches!(self, UpdateCheckStatus::NoUpdate)
    }
}

impl Display for UpdateCheckStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            UpdateCheckStatus::Ok => f.write_str("ok"),
            UpdateCheckStatus::NoUpdate => f.write_str("noupdate"),
            UpdateCheckStatus::ErrorInternal => f.write_str("error-internal"),
            UpdateCheckStatus::Other(s) => f.write_str(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_status_known_values() {
        assert_eq!(serde_plain_from(r#""ok""#), AppStatus::Ok,);
        assert_eq!(
            serde_plain_from(r#""error-updateInProgressOnInstance""#),
            AppStatus::UpdateInProgress,
        );
    }

    #[test]
    fn app_status_unknown_does_not_fail() {
        // The critical property: an unrecognised status must deserialize into
        // `Other`, never error.
        let status = serde_plain_from(r#""error-somethingBrandNew""#);
        assert_eq!(
            status,
            AppStatus::Other("error-somethingBrandNew".to_string())
        );
        assert!(!status.is_ok());
        assert!(!status.is_update_in_progress());
    }

    #[test]
    fn update_check_status_unknown_does_not_fail() {
        let status: UpdateCheckStatus = serde_json::from_str(r#""error-brandNew""#).unwrap();
        assert_eq!(
            status,
            UpdateCheckStatus::Other("error-brandNew".to_string())
        );
    }

    /// Helper: deserialize an `AppStatus` from a JSON string literal (serde data
    /// model is shared with XML attribute deserialization).
    fn serde_plain_from(s: &str) -> AppStatus {
        serde_json::from_str(s).unwrap()
    }
}
