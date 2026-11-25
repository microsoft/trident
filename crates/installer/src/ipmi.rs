//! Reports live installation-attempt outcomes to the local BMC.

use anyhow::Error;
use log::{debug, warn};

use osutils::ipmi::{self, SendOutcome};

use crate::client::AttemptOutcome;

/// Generator ID used by `ipmitool` for system-management software events.
const IPMI_SYSTEM_MANAGEMENT_SOFTWARE_GENERATOR_ID: u8 = 0x41;
/// Event Message revision required by the IPMI installation-status payload.
const IPMI_EVENT_MESSAGE_REVISION: u8 = 0x04;
/// IPMI sensor type for base OS boot and installation status.
const IPMI_OS_BOOT_SENSOR_TYPE: u8 = 0x1f;
/// Sensor number reserved for software-generated installation status.
const IPMI_SENSOR_NUMBER: u8 = 0x00;
/// Sensor-specific assertion event type used by the installation statuses.
const IPMI_SENSOR_SPECIFIC_EVENT_TYPE: u8 = 0x6f;
/// IPMI event code for an installation attempt starting.
const IPMI_INSTALLATION_STARTED_EVENT: u8 = 0x07;
/// IPMI event code for an installation attempt completing successfully.
const IPMI_INSTALLATION_COMPLETED_EVENT: u8 = 0x08;
/// IPMI event code for an installation failure.
const IPMI_INSTALLATION_FAILED_EVENT: u8 = 0x0a;

/// Installer lifecycle state reported to the local BMC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstallerStatus {
    /// A live installation attempt began.
    Started,
    /// Trident reported a successful terminal result.
    Completed,
    /// The installation encountered an error or lost its worker unexpectedly.
    Failed,
}

/// Abstracts status delivery so lifecycle behavior can be tested without IPMI.
trait StatusReporter {
    /// Reports one lifecycle state without changing the installation outcome.
    fn report(&mut self, status: InstallerStatus) -> Result<SendOutcome, Error>;
}

/// Sends installer status through the local Linux IPMI interface.
struct LocalStatusReporter;

impl StatusReporter for LocalStatusReporter {
    fn report(&mut self, status: InstallerStatus) -> Result<SendOutcome, Error> {
        ipmi::send_platform_event(payload(status))
    }
}

/// Tracks one attempt and emits at most one start and one terminal event.
struct AttemptReporter<R: StatusReporter> {
    /// Delivers lifecycle events to the selected transport.
    reporter: R,
    /// Records whether a start lacks a corresponding terminal outcome.
    active: bool,
}

impl<R: StatusReporter> AttemptReporter<R> {
    /// Creates an inactive reporter for one installation attempt.
    fn new(reporter: R) -> Self {
        Self {
            reporter,
            active: false,
        }
    }

    /// Reports the start once and marks the attempt active.
    fn start(&mut self) {
        if self.active {
            return;
        }
        self.emit(InstallerStatus::Started);
        self.active = true;
    }

    /// Reports a handled terminal outcome once and marks the attempt inactive.
    fn finish(&mut self, outcome: AttemptOutcome) {
        if !self.active {
            return;
        }
        self.active = false;
        self.emit(match outcome {
            AttemptOutcome::Completed => InstallerStatus::Completed,
            AttemptOutcome::Failed => InstallerStatus::Failed,
        });
    }

    /// Reports failure when an active attempt exits without a handled outcome.
    fn fail(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.emit(InstallerStatus::Failed);
    }

    /// Attempts delivery while keeping all reporting failures non-fatal.
    fn emit(&mut self, status: InstallerStatus) {
        match self.reporter.report(status) {
            Ok(SendOutcome::Sent) => {}
            Ok(SendOutcome::Unavailable) => {
                debug!("Local IPMI installer status reporting is unavailable");
            }
            Ok(_) => {}
            Err(error) => {
                warn!("Failed to report installer status {status:?} through local IPMI: {error:#}");
            }
        }
    }
}

impl<R: StatusReporter> Drop for AttemptReporter<R> {
    /// Converts an unclosed active attempt into a failed lifecycle event.
    fn drop(&mut self) {
        self.fail();
    }
}

/// Live-attempt reporter backed by the local Linux IPMI interface.
pub(crate) struct LiveAttemptReporter {
    /// Owns the lifecycle state and local transport implementation.
    inner: AttemptReporter<LocalStatusReporter>,
}

impl LiveAttemptReporter {
    /// Creates an inactive best-effort local IPMI reporter.
    pub(crate) fn new() -> Self {
        Self {
            inner: AttemptReporter::new(LocalStatusReporter),
        }
    }

    /// Reports the start of a live installation attempt.
    pub(crate) fn start(&mut self) {
        self.inner.start();
    }

    /// Reports the handled terminal outcome of a live installation attempt.
    pub(crate) fn finish(&mut self, outcome: AttemptOutcome) {
        self.inner.finish(outcome);
    }
}

/// Encodes an installer lifecycle state as an IPMI installation-status event payload.
const fn payload(status: InstallerStatus) -> [u8; 8] {
    let event = match status {
        InstallerStatus::Started => IPMI_INSTALLATION_STARTED_EVENT,
        InstallerStatus::Completed => IPMI_INSTALLATION_COMPLETED_EVENT,
        InstallerStatus::Failed => IPMI_INSTALLATION_FAILED_EVENT,
    };
    [
        IPMI_SYSTEM_MANAGEMENT_SOFTWARE_GENERATOR_ID,
        IPMI_EVENT_MESSAGE_REVISION,
        IPMI_OS_BOOT_SENSOR_TYPE,
        IPMI_SENSOR_NUMBER,
        IPMI_SENSOR_SPECIFIC_EVENT_TYPE,
        event,
        0,
        0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{cell::RefCell, rc::Rc};

    use anyhow::anyhow;

    #[derive(Clone, Copy)]
    enum MockReportResult {
        Sent,
        Unavailable,
        Error,
    }

    struct MockStatusReporter {
        statuses: Rc<RefCell<Vec<InstallerStatus>>>,
        result: MockReportResult,
    }

    impl StatusReporter for MockStatusReporter {
        fn report(&mut self, status: InstallerStatus) -> Result<SendOutcome, Error> {
            self.statuses.borrow_mut().push(status);
            match self.result {
                MockReportResult::Sent => Ok(SendOutcome::Sent),
                MockReportResult::Unavailable => Ok(SendOutcome::Unavailable),
                MockReportResult::Error => Err(anyhow!("injected status reporting failure")),
            }
        }
    }

    fn mock_reporter(
        result: MockReportResult,
    ) -> (MockStatusReporter, Rc<RefCell<Vec<InstallerStatus>>>) {
        let statuses = Rc::new(RefCell::new(Vec::new()));
        (
            MockStatusReporter {
                statuses: Rc::clone(&statuses),
                result,
            },
            statuses,
        )
    }

    #[test]
    fn statuses_use_ipmi_payload_bytes() {
        for (status, event) in [
            (InstallerStatus::Started, IPMI_INSTALLATION_STARTED_EVENT),
            (
                InstallerStatus::Completed,
                IPMI_INSTALLATION_COMPLETED_EVENT,
            ),
            (InstallerStatus::Failed, IPMI_INSTALLATION_FAILED_EVENT),
        ] {
            assert_eq!(
                payload(status),
                [0x41, 0x04, 0x1f, 0x00, 0x6f, event, 0x00, 0x00]
            );
        }
    }

    #[test]
    fn live_attempt_reports_one_start_and_terminal_outcome() {
        let (reporter, statuses) = mock_reporter(MockReportResult::Sent);
        {
            let mut attempt = AttemptReporter::new(reporter);
            attempt.start();
            attempt.start();
            attempt.finish(AttemptOutcome::Completed);
        }
        assert_eq!(
            *statuses.borrow(),
            [InstallerStatus::Started, InstallerStatus::Completed]
        );

        let (reporter, statuses) = mock_reporter(MockReportResult::Sent);
        {
            let mut attempt = AttemptReporter::new(reporter);
            attempt.start();
            attempt.finish(AttemptOutcome::Failed);
        }
        assert_eq!(
            *statuses.borrow(),
            [InstallerStatus::Started, InstallerStatus::Failed]
        );
    }

    #[test]
    fn active_attempt_drop_reports_failed() {
        let (reporter, statuses) = mock_reporter(MockReportResult::Sent);
        {
            let mut attempt = AttemptReporter::new(reporter);
            attempt.start();
        }
        assert_eq!(
            *statuses.borrow(),
            [InstallerStatus::Started, InstallerStatus::Failed]
        );
    }

    #[test]
    fn reporting_failures_do_not_change_attempt_outcome() {
        for result in [MockReportResult::Unavailable, MockReportResult::Error] {
            let (reporter, statuses) = mock_reporter(result);
            {
                let mut attempt = AttemptReporter::new(reporter);
                attempt.start();
                attempt.finish(AttemptOutcome::Failed);
            }
            assert_eq!(
                *statuses.borrow(),
                [InstallerStatus::Started, InstallerStatus::Failed]
            );
        }
    }
}
