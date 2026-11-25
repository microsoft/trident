//! Reports live installation-attempt outcomes to the local BMC.

use anyhow::Error;
use log::{debug, warn};

use osutils::ipmi::{InstallationStatus, SendOutcome};

use crate::client::AttemptOutcome;

/// Abstracts status delivery so lifecycle behavior can be tested without IPMI.
trait StatusReporter {
    /// Reports one lifecycle state without changing the installation outcome.
    fn report(&mut self, status: InstallationStatus) -> Result<SendOutcome, Error>;
}

/// Sends installer status through the local Linux IPMI interface.
struct LocalStatusReporter;

#[cfg(not(test))]
impl StatusReporter for LocalStatusReporter {
    fn report(&mut self, status: InstallationStatus) -> Result<SendOutcome, Error> {
        use osutils::ipmi;

        ipmi::send_installation_status(status)
    }
}

/// Unit tests compile the live reporter with a no-I/O backend.
#[cfg(test)]
impl StatusReporter for LocalStatusReporter {
    fn report(&mut self, _status: InstallationStatus) -> Result<SendOutcome, Error> {
        Ok(SendOutcome::Sent)
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
        self.emit(InstallationStatus::Started);
        self.active = true;
    }

    /// Reports a handled terminal outcome once and marks the attempt inactive.
    fn finish(&mut self, outcome: AttemptOutcome) {
        if !self.active {
            return;
        }
        self.active = false;
        self.emit(match outcome {
            AttemptOutcome::Completed => InstallationStatus::Completed,
            AttemptOutcome::Failed => InstallationStatus::Failed,
        });
    }

    /// Reports failure when an active attempt exits without a handled outcome.
    fn fail(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.emit(InstallationStatus::Failed);
    }

    /// Attempts delivery while keeping all reporting failures non-fatal.
    fn emit(&mut self, status: InstallationStatus) {
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
        statuses: Rc<RefCell<Vec<InstallationStatus>>>,
        result: MockReportResult,
    }

    impl StatusReporter for MockStatusReporter {
        fn report(&mut self, status: InstallationStatus) -> Result<SendOutcome, Error> {
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
    ) -> (MockStatusReporter, Rc<RefCell<Vec<InstallationStatus>>>) {
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
            [InstallationStatus::Started, InstallationStatus::Completed]
        );

        let (reporter, statuses) = mock_reporter(MockReportResult::Sent);
        {
            let mut attempt = AttemptReporter::new(reporter);
            attempt.start();
            attempt.finish(AttemptOutcome::Failed);
        }
        assert_eq!(
            *statuses.borrow(),
            [InstallationStatus::Started, InstallationStatus::Failed]
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
            [InstallationStatus::Started, InstallationStatus::Failed]
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
                [InstallationStatus::Started, InstallationStatus::Failed]
            );
        }
    }

    #[test]
    fn live_reporter_uses_unit_test_backend() {
        let mut reporter = LocalStatusReporter;
        assert_eq!(
            reporter.report(InstallationStatus::Started).unwrap(),
            SendOutcome::Sent
        );
    }
}
