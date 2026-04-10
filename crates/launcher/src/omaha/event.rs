use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct OmahaEvent {
    #[serde(rename = "@eventtype")]
    pub(crate) event_type: OmahaEventType,

    #[serde(rename = "@eventresult")]
    pub(crate) event_result: EventResult,
}

impl OmahaEvent {
    pub fn new(event_type: OmahaEventType, event_result: EventResult) -> Self {
        Self {
            event_type,
            event_result,
        }
    }
}

/// Event types for Omaha events.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum OmahaEventType {
    #[serde(rename = "0")]
    Unknown,

    #[serde(rename = "1")]
    DownloadComplete,

    #[serde(rename = "2")]
    InstallComplete,

    #[serde(rename = "3")]
    UpdateComplete,

    #[serde(rename = "4")]
    Uninstall,

    #[serde(rename = "5")]
    DownloadStarted,

    #[serde(rename = "6")]
    InstallStarted,

    #[serde(rename = "10")]
    SetupStarted,

    #[serde(rename = "11")]
    SetupFinished,

    #[serde(rename = "12")]
    UpdateApplicationStarted,

    #[serde(rename = "13")]
    UpdateDownloadStarted,

    #[serde(rename = "14")]
    UpdateDownloadFinished,

    /// Custom value defined by Nebraska.
    #[serde(rename = "800")]
    EventUpdateInstalled,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum EventResult {
    #[serde(rename = "0")]
    Error,

    #[serde(rename = "1")]
    Success,

    #[serde(rename = "2")]
    SuccessReboot,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EventAcknowledge {
    #[serde(rename = "@status")]
    event: EventAcknowledgeStatus,
}

impl EventAcknowledge {
    pub(crate) fn is_ok(&self) -> bool {
        matches!(self.event, EventAcknowledgeStatus::Ok)
    }
}

#[derive(Debug, Deserialize)]
pub enum EventAcknowledgeStatus {
    #[serde(rename = "ok")]
    Ok,

    #[serde(other)]
    Unknown,
}
