//! Reports live installation-attempt outcomes to the local BMC.

use anyhow::Error;
use log::{debug, warn};

use osutils::ipmi::{self, SendOutcome};

use crate::client::AttemptOutcome;

const ANACONDA_GENERATOR_ID: u8 = 0x41;
const IPMI_EVENT_MESSAGE_REVISION: u8 = 0x04;
const IPMI_OS_BOOT_SENSOR_TYPE: u8 = 0x1f;
const IPMI_SENSOR_NUMBER: u8 = 0x00;
const IPMI_SENSOR_SPECIFIC_EVENT_TYPE: u8 = 0x6f;
const ANACONDA_STARTED_EVENT: u8 = 0x07;
const ANACONDA_COMPLETED_EVENT: u8 = 0x08;
const ANACONDA_ABORTED_EVENT: u8 = 0x09;
const ANACONDA_FAILED_EVENT: u8 = 0x0a;

/// Installer lifecycle state reported to the local BMC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InstallerStatus {
    /// A live installation attempt began.
    Started,
    /// Trident reported a successful terminal result.
    Completed,
    /// The attempt ended through a handled preparation or servicing error.
    Aborted,
    /// The attempt disappeared without a handled terminal result.
    Failed,
}

pub(super) trait StatusReporter {
    fn report(&mut self, status: InstallerStatus) -> Result<SendOutcome, Error>;
}

/// Sends installer status through the local Linux IPMI interface.
pub(super) struct LocalStatusReporter;

impl StatusReporter for LocalStatusReporter {
    fn report(&mut self, status: InstallerStatus) -> Result<SendOutcome, Error> {
        ipmi::send_platform_event(payload(status))
    }
}

/// Ensures each live attempt produces one start and one terminal event.
pub(super) struct LiveAttemptReporter<R: StatusReporter> {
    reporter: R,
    active: bool,
}

impl<R: StatusReporter> LiveAttemptReporter<R> {
    pub(super) fn new(reporter: R) -> Self {
        Self {
            reporter,
            active: false,
        }
    }

    pub(super) fn start(&mut self) {
        if self.active {
            return;
        }
        self.emit(InstallerStatus::Started);
        self.active = true;
    }

    pub(super) fn finish(&mut self, outcome: AttemptOutcome) {
        if !self.active {
            return;
        }
        self.active = false;
        self.emit(match outcome {
            AttemptOutcome::Completed => InstallerStatus::Completed,
            AttemptOutcome::Aborted => InstallerStatus::Aborted,
        });
    }

    fn fail(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.emit(InstallerStatus::Failed);
    }

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

impl<R: StatusReporter> Drop for LiveAttemptReporter<R> {
    fn drop(&mut self) {
        self.fail();
    }
}

const fn payload(status: InstallerStatus) -> [u8; 8] {
    let event = match status {
        InstallerStatus::Started => ANACONDA_STARTED_EVENT,
        InstallerStatus::Completed => ANACONDA_COMPLETED_EVENT,
        InstallerStatus::Aborted => ANACONDA_ABORTED_EVENT,
        InstallerStatus::Failed => ANACONDA_FAILED_EVENT,
    };
    [
        ANACONDA_GENERATOR_ID,
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
    fn statuses_use_anaconda_payload_bytes() {
        for (status, event) in [
            (InstallerStatus::Started, ANACONDA_STARTED_EVENT),
            (InstallerStatus::Completed, ANACONDA_COMPLETED_EVENT),
            (InstallerStatus::Aborted, ANACONDA_ABORTED_EVENT),
            (InstallerStatus::Failed, ANACONDA_FAILED_EVENT),
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
            let mut attempt = LiveAttemptReporter::new(reporter);
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
            let mut attempt = LiveAttemptReporter::new(reporter);
            attempt.start();
            attempt.finish(AttemptOutcome::Aborted);
        }
        assert_eq!(
            *statuses.borrow(),
            [InstallerStatus::Started, InstallerStatus::Aborted]
        );
    }

    #[test]
    fn active_attempt_drop_reports_failed() {
        let (reporter, statuses) = mock_reporter(MockReportResult::Sent);
        {
            let mut attempt = LiveAttemptReporter::new(reporter);
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
                let mut attempt = LiveAttemptReporter::new(reporter);
                attempt.start();
                attempt.finish(AttemptOutcome::Aborted);
            }
            assert_eq!(
                *statuses.borrow(),
                [InstallerStatus::Started, InstallerStatus::Aborted]
            );
        }
    }
}
