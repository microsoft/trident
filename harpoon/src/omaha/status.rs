use std::fmt::{self, Display, Formatter};

use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum AppStatus {
    #[serde(rename = "ok")]
    Ok,

    #[serde(rename = "restricted")]
    Restricted,

    #[serde(rename = "error-unknownApplication")]
    ErrorUnknownApplication,

    #[serde(rename = "error-invalidAppId")]
    ErrorInvalidAppId,

    #[serde(untagged)]
    Other(String),
}

impl AppStatus {
    pub(crate) fn is_error(&self) -> bool {
        !matches!(self, AppStatus::Ok)
    }
}

impl Display for AppStatus {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            AppStatus::Ok => write!(f, "ok"),
            AppStatus::Restricted => write!(f, "restricted"),
            AppStatus::ErrorUnknownApplication => write!(f, "error-unknownApplication"),
            AppStatus::ErrorInvalidAppId => write!(f, "error-invalidAppId"),
            AppStatus::Other(other) => write!(f, "other: {}", other),
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum UpdateCheckStatus {
    #[serde(rename = "noupdate")]
    NoUpdate,

    #[serde(rename = "ok")]
    Ok,

    #[serde(rename = "error-osnotsupported")]
    ErrorOsNotSupported,

    #[serde(rename = "error-unsupportedProtocol")]
    ErrorUnsupportedProtocol,

    #[serde(rename = "error-pluginRestrictedHost")]
    ErrorPluginRestrictedHost,

    #[serde(rename = "error-hash")]
    ErrorHash,

    #[serde(rename = "error-internal")]
    ErrorInternal,

    #[serde(untagged)]
    Other(String),
}

impl UpdateCheckStatus {
    pub(crate) fn is_error(&self) -> bool {
        !matches!(self, UpdateCheckStatus::NoUpdate | UpdateCheckStatus::Ok)
    }

    pub(crate) fn is_no_update(&self) -> bool {
        matches!(self, UpdateCheckStatus::NoUpdate)
    }
}

impl Display for UpdateCheckStatus {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            UpdateCheckStatus::NoUpdate => write!(f, "noupdate"),
            UpdateCheckStatus::Ok => write!(f, "ok"),
            UpdateCheckStatus::ErrorOsNotSupported => write!(f, "error-osnotsupported"),
            UpdateCheckStatus::ErrorUnsupportedProtocol => write!(f, "error-unsupportedProtocol"),
            UpdateCheckStatus::ErrorPluginRestrictedHost => write!(f, "error-pluginRestrictedHost"),
            UpdateCheckStatus::ErrorHash => write!(f, "error-hash"),
            UpdateCheckStatus::ErrorInternal => write!(f, "error-internal"),
            UpdateCheckStatus::Other(other) => write!(f, "other: {}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_status_display() {
        assert_eq!(AppStatus::Ok.to_string(), "ok");
        assert_eq!(AppStatus::Restricted.to_string(), "restricted");
        assert_eq!(
            AppStatus::ErrorUnknownApplication.to_string(),
            "error-unknownApplication"
        );
        assert_eq!(
            AppStatus::ErrorInvalidAppId.to_string(),
            "error-invalidAppId"
        );
        assert_eq!(
            AppStatus::Other("other".to_string()).to_string(),
            "other: other"
        );
    }

    #[test]
    fn test_update_check_status_display() {
        assert_eq!(UpdateCheckStatus::NoUpdate.to_string(), "noupdate");
        assert_eq!(UpdateCheckStatus::Ok.to_string(), "ok");
        assert_eq!(
            UpdateCheckStatus::ErrorOsNotSupported.to_string(),
            "error-osnotsupported"
        );
        assert_eq!(
            UpdateCheckStatus::ErrorUnsupportedProtocol.to_string(),
            "error-unsupportedProtocol"
        );
        assert_eq!(
            UpdateCheckStatus::ErrorPluginRestrictedHost.to_string(),
            "error-pluginRestrictedHost"
        );
        assert_eq!(UpdateCheckStatus::ErrorHash.to_string(), "error-hash");
        assert_eq!(
            UpdateCheckStatus::ErrorInternal.to_string(),
            "error-internal"
        );
        assert_eq!(
            UpdateCheckStatus::Other("other".to_string()).to_string(),
            "other: other"
        );
    }

    #[test]
    fn test_flag_methods() {
        assert!(!AppStatus::Ok.is_error());
        assert!(AppStatus::Restricted.is_error());
        assert!(AppStatus::ErrorUnknownApplication.is_error());
        assert!(AppStatus::ErrorInvalidAppId.is_error());
        assert!(AppStatus::Other("other".to_string()).is_error());

        assert!(UpdateCheckStatus::NoUpdate.is_no_update());
        assert!(!UpdateCheckStatus::Ok.is_no_update());
        assert!(!UpdateCheckStatus::ErrorOsNotSupported.is_no_update());
        assert!(!UpdateCheckStatus::ErrorUnsupportedProtocol.is_no_update());
        assert!(!UpdateCheckStatus::ErrorPluginRestrictedHost.is_no_update());
        assert!(!UpdateCheckStatus::ErrorHash.is_no_update());
        assert!(!UpdateCheckStatus::ErrorInternal.is_no_update());
        assert!(!UpdateCheckStatus::Other("other".to_string()).is_no_update());

        assert!(!UpdateCheckStatus::NoUpdate.is_error());
        assert!(!UpdateCheckStatus::Ok.is_error());
        assert!(UpdateCheckStatus::ErrorOsNotSupported.is_error());
        assert!(UpdateCheckStatus::ErrorUnsupportedProtocol.is_error());
        assert!(UpdateCheckStatus::ErrorPluginRestrictedHost.is_error());
        assert!(UpdateCheckStatus::ErrorHash.is_error());
        assert!(UpdateCheckStatus::ErrorInternal.is_error());
        assert!(UpdateCheckStatus::Other("other".to_string()).is_error());
    }
}
