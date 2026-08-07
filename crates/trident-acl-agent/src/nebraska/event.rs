//! Omaha update events, constrained to the pairs Nebraska actually accepts.
//!
//! Nebraska validates each event against a **whitelist** of `(eventtype,
//! eventresult)` pairs seeded in its `event_type` table. Only six pairs exist:
//!
//! ```text
//! (3,0)  (3,1)  (3,2)  (13,1)  (14,1)  (800,1)
//! ```
//!
//! An event with any other pair is **silently discarded** — Nebraska still
//! returns `<event status="ok">`, so the client cannot detect the mistake from
//! the response. To make that class of bug impossible, this module never
//! exposes raw integers: callers work with typed events, and the mapping to
//! wire values is private and total over the whitelist.
//!
//! The events also split into two kinds with very different consequences:
//!
//! - **Progress** events ([`ProgressEvent`]) are informational. Sending them is
//!   a *commitment* to also send a terminal event, because leaving an instance
//!   in a progress state (Downloading/Downloaded/Installed) wedges it
//!   permanently.
//! - **Terminal** events move the instance to a final state. They are not
//!   exposed as free-standing values precisely so a caller cannot send one in
//!   the wrong shape or context; they are emitted only through the dedicated
//!   [`Client`](crate::nebraska::Client) methods
//!   (`complete_after_reboot`, `report_failure`).

/// The `eventtype`/`eventresult` wire pair for an Omaha event.
///
/// Constructed only by this module, and only ever with whitelisted values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WirePair {
    pub(super) event_type: u16,
    pub(super) event_result: u8,
}

/// A progress event reported while an update is being applied.
///
/// These correspond to the intermediate Nebraska instance states. Emitting any
/// of them commits the caller to eventually reporting a terminal event (success
/// or failure); see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressEvent {
    /// Staging of the update has begun. Wire `(13, 1)` → Nebraska `Downloading`.
    DownloadStarted,
    /// Staging of the update has finished. Wire `(14, 1)` → Nebraska `Downloaded`.
    DownloadFinished,
    /// The update has been finalized and the new slot armed. Wire `(800, 1)` →
    /// Nebraska `Installed`.
    Installed,
}

impl ProgressEvent {
    /// The whitelisted wire pair for this progress event.
    pub(super) fn wire(self) -> WirePair {
        match self {
            ProgressEvent::DownloadStarted => WirePair {
                event_type: 13,
                event_result: 1,
            },
            ProgressEvent::DownloadFinished => WirePair {
                event_type: 14,
                event_result: 1,
            },
            ProgressEvent::Installed => WirePair {
                event_type: 800,
                event_result: 1,
            },
        }
    }

    /// A short human-readable label, suitable for operator-facing logging.
    pub fn label(self) -> &'static str {
        match self {
            ProgressEvent::DownloadStarted => "download started",
            ProgressEvent::DownloadFinished => "download finished",
            ProgressEvent::Installed => "installed",
        }
    }
}

/// A terminal event, moving the instance to a final state.
///
/// Not publicly constructible: terminal events are emitted only by the
/// [`Client`](crate::nebraska::Client) so they always carry the correct
/// surrounding request shape (e.g. the batched update-check that completion
/// requires).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalEvent {
    /// The update completed successfully after reboot. Wire `(3, 2)` (the
    /// better-tested "success + reboot" branch for non-Flatcar apps) → Nebraska
    /// `Complete`.
    Completed,
    /// The update failed. Wire `(3, 0)` → Nebraska `Error`; clears
    /// `update_in_progress` and re-arms the instance so a later check can grant
    /// again.
    Failed,
}

impl TerminalEvent {
    pub(super) fn wire(self) -> WirePair {
        match self {
            TerminalEvent::Completed => WirePair {
                event_type: 3,
                event_result: 2,
            },
            TerminalEvent::Failed => WirePair {
                event_type: 3,
                event_result: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact set of pairs seeded in Nebraska's `event_type` table.
    const WHITELIST: &[(u16, u8)] = &[(3, 0), (3, 1), (3, 2), (13, 1), (14, 1), (800, 1)];

    fn is_whitelisted(p: WirePair) -> bool {
        WHITELIST.contains(&(p.event_type, p.event_result))
    }

    #[test]
    fn progress_events_are_whitelisted() {
        for ev in [
            ProgressEvent::DownloadStarted,
            ProgressEvent::DownloadFinished,
            ProgressEvent::Installed,
        ] {
            assert!(is_whitelisted(ev.wire()), "{ev:?} -> {:?}", ev.wire());
        }
    }

    #[test]
    fn terminal_events_are_whitelisted() {
        for ev in [TerminalEvent::Completed, TerminalEvent::Failed] {
            assert!(is_whitelisted(ev.wire()), "{ev:?} -> {:?}", ev.wire());
        }
    }

    #[test]
    fn wire_values_match_spec() {
        assert_eq!(
            ProgressEvent::DownloadStarted.wire(),
            WirePair {
                event_type: 13,
                event_result: 1
            }
        );
        assert_eq!(
            ProgressEvent::DownloadFinished.wire(),
            WirePair {
                event_type: 14,
                event_result: 1
            }
        );
        assert_eq!(
            ProgressEvent::Installed.wire(),
            WirePair {
                event_type: 800,
                event_result: 1
            }
        );
        assert_eq!(
            TerminalEvent::Completed.wire(),
            WirePair {
                event_type: 3,
                event_result: 2
            }
        );
        assert_eq!(
            TerminalEvent::Failed.wire(),
            WirePair {
                event_type: 3,
                event_result: 0
            }
        );
    }
}
