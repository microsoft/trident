package validate

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
	tridentutil "tridenttools/storm/utils/trident"
)

// Health-check rollback failure log location and the messages the
// health-checks-install scenario is expected to produce. These strings are
// properties of that scenario's Host Configuration health checks (scripts
// `invoke-rollback-from-script` referencing two non-existent services), mirrored
// from rollback_test.py.
const (
	healthCheckFailureLogGlob = "/var/lib/trident/trident-health-check-failure-*.log"
	rollbackFailedHealthError = "Failed health check(s)"
)

var expectedRollbackLogMessages = []string{
	"Script 'invoke-rollback-from-script' failed",
	"Unit non-existent-service1.service could not be found",
	"Unit non-existent-service2.service could not be found",
}

// HasRollbackIntent reports whether the scenario is expected to trigger a
// health-check rollback, signalled by a top-level `health` section in the Host
// Configuration. Such scenarios replace `base` validation with rollback
// validation.
func HasRollbackIntent(hs tridentutil.HostStatus) bool {
	return hs.Spec().Exists("health")
}

// ValidateRollback ports rollback_test.py::test_rollback. It confirms the host
// reached the expected (rolled-back) servicing state, that the last error
// reflects a failed health check, that the active volume is unchanged (or
// absent when not provisioned), and that the health-check failure log records
// the expected script/service failures.
func ValidateRollback(
	sa *SoftAsserter,
	client *ssh.Client,
	hs tridentutil.HostStatus,
	expectedState tridentutil.ServicingState,
	abActive tridentutil.AbVolumeSelection,
) {
	sa.Assert("rollback/servicing-state",
		hs.ServicingState() == expectedState,
		"expected servicingState %q, got %q", expectedState, hs.ServicingState())

	if lastErr, ok := hs.LastError(); ok {
		sa.Assert("rollback/last-error",
			strings.Contains(lastErr, rollbackFailedHealthError),
			"lastError does not contain %q: %s", rollbackFailedHealthError, lastErr)
	} else {
		sa.Failf("rollback/last-error", "expected a lastError reflecting a failed health check")
	}

	if expectedState == tridentutil.ServicingStateNotProvisioned {
		if _, present := hs.AbActiveVolume(); present {
			sa.Failf("rollback/active-volume", "abActiveVolume should be absent when not provisioned")
		}
	} else {
		actual, present := hs.AbActiveVolume()
		sa.Assert("rollback/active-volume",
			present && actual == abActive,
			"expected abActiveVolume %q, got %q (present=%v)", abActive, actual, present)
	}

	validateRollbackLogs(sa, client)
}

// validateRollbackLogs checks that exactly one health-check failure log exists
// and that it records the expected failure messages.
func validateRollbackLogs(sa *SoftAsserter, client *ssh.Client) {
	listOut, err := sshutils.CommandOutput(client, "sudo ls "+healthCheckFailureLogGlob)
	if err != nil {
		sa.Fail("rollback/log-list", err)
		return
	}

	logFiles := strings.Fields(strings.TrimSpace(listOut))
	if len(logFiles) != 1 {
		sa.Failf("rollback/log-count", "expected exactly 1 health-check failure log, found %d: %v",
			len(logFiles), logFiles)
		return
	}

	content, err := sshutils.CommandOutput(client, fmt.Sprintf("sudo cat %s", logFiles[0]))
	if err != nil {
		sa.Fail("rollback/log-read", err)
		return
	}

	for _, want := range expectedRollbackLogMessages {
		sa.Assert("rollback/log-message",
			strings.Contains(content, want),
			"health-check failure log missing message %q", want)
	}
}
