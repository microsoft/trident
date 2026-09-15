package scenario

import (
	"tridenttools/storm/utils/screenshot"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

// screenshotArtifact is where a capture is published, relative to the run's
// artifact directory.
const (
	installScreenshotArtifact = "install/screen.png"
	failureScreenshotArtifact = "screen-failure.png"
)

// captureScreenshot publishes a PNG of the VM console as a run artifact.
//
// Failures are logged and swallowed: a screenshot is a diagnostic aid, and
// losing one must never turn a passing case red, nor mask the real error in a
// case that is already failing.
func (s *TridentE2EScenario) captureScreenshot(tc storm.TestCase, artifact string) {
	if s.testHost == nil {
		return
	}
	vm := s.testHost.VmInfo()
	if vm == nil {
		// Nothing to photograph: bare metal, or the VM was never created.
		return
	}

	writer := tc.ArtifactBroker().StreamArtifactData(artifact)
	if writer == nil {
		return
	}
	defer writer.Close()

	if err := screenshot.CapturePng(vm.Lv(), vm.LvDomain(), writer); err != nil {
		logrus.Warnf("Failed to capture VM screenshot to %q: %v", artifact, err)
		return
	}
	logrus.Infof("Captured VM console screenshot to %q", artifact)
}

// withFailureScreenshot wraps a test case so that a console screenshot is
// published whenever the case does not complete successfully.
//
// Legacy only captured once, unconditionally, after the clean install. Capturing
// at the point of failure is what actually helps: for the post-reboot SSH
// reconnect timeouts this suite hits, the console immediately distinguishes a
// slow boot from a kernel panic or an emergency shell.
//
// A case can end in three ways: returning an error, calling tc.Fail*/tc.Error
// (which call runtime.Goexit), or calling tc.Skip (also Goexit). The deferred
// check covers all of them, and skips are excluded explicitly so the routinely
// skipped cases do not litter the artifact directory.
func (s *TridentE2EScenario) withFailureScreenshot(fn storm.TestCaseFunction) storm.TestCaseFunction {
	return func(tc storm.TestCase) error {
		completed := false
		s.caseSkipped = false

		defer func() {
			if completed || s.caseSkipped {
				return
			}
			// The case is unwinding via Goexit or a panic, i.e. it failed.
			s.captureScreenshot(tc, failureScreenshotArtifact)
		}()

		err := fn(tc)
		completed = true
		if err != nil {
			s.captureScreenshot(tc, failureScreenshotArtifact)
		}
		return err
	}
}

// markSkipped records that the current case is ending as a skip rather than a
// failure, so withFailureScreenshot does not photograph a healthy host.
func (s *TridentE2EScenario) markSkipped() {
	s.caseSkipped = true
}
