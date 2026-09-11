package validate

import (
	"fmt"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/pkg/hostconfig"
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

// Health checks the E2E scenario injects into every configuration to validate
// UEFI fallback. They are expected to PASS, so they must never be read as a
// signal that a configuration means to fail its health checks.
const (
	UefiFallbackInstallCheckName  = "uefi-fallback-validation-install"
	UefiFallbackAbUpdateCheckName = "uefi-fallback-validation-update"
)

// HasRollbackIntent reports whether the scenario is expected to trigger a
// health-check rollback. Such scenarios replace `base` validation with rollback
// validation.
func HasRollbackIntent(hs tridentutil.HostStatus) bool {
	return HasFailingHealthChecks(hs.Spec())
}

// HasFailingHealthChecks reports whether a Host Configuration declares health
// checks that are meant to fail.
//
// The signal used to be simply "a top-level health section exists", which held
// while the only configuration declaring one was the rollback-intent
// configuration. The scenario now injects UEFI fallback checks into every
// configuration, so the checks themselves have to be inspected: counting ours
// would make every configuration look like it intends to roll back, which in
// turn makes a clean install expect a failed Trident service and swaps base
// validation for rollback validation everywhere.
func HasFailingHealthChecks(config hostconfig.HostConfig) bool {
	if !config.Exists("health") {
		return false
	}

	checks := config.S("health", "checks")
	if checks == nil {
		// A health section that declares no checks at all: keep the original,
		// broader signal rather than silently deciding there is no intent.
		return true
	}

	for _, check := range checks.Children() {
		name, _ := check.S("name").Data().(string)
		if name != UefiFallbackInstallCheckName && name != UefiFallbackAbUpdateCheckName {
			return true
		}
	}
	return false
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
