package scenario

import (
	"fmt"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"

	"tridenttools/storm/utils/trident"
)

// validateRollback validates health check rollback behavior on the remote host.
// Converted from rollback_test.py test_rollback.
//
// It validates:
//   - servicingState matches expected state based on scenario context
//   - lastError contains "Failed health check(s)"
//   - abActiveVolume is absent (not-provisioned) or unchanged (after A/B update)
//   - Exactly one health-check-failure log file exists
//   - Log file contains expected failure messages
func (s *TridentE2EScenario) validateRollback(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	// Get host status via trident get
	tridentOut, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "get")
	if err != nil {
		return fmt.Errorf("failed to run trident get: %w", err)
	}

	if tridentOut.Status != 0 {
		return fmt.Errorf("trident get failed with status %d: %s",
			tridentOut.Status, tridentOut.Stderr)
	}

	hostStatus, err := ParseTridentGetOutput(tridentOut.Stdout)
	if err != nil {
		return fmt.Errorf("failed to parse trident get output: %w", err)
	}

	// Determine expected state based on scenario context.
	// For health-checks-install (no A/B update), health check failures during
	// clean install result in "not-provisioned" state.
	// For scenarios with A/B update, the state remains "provisioned".
	expectedState := "not-provisioned"
	if s.originalConfig.HasABUpdate() {
		expectedState = "provisioned"
	}

	// Validate servicingState
	servicingState, _ := hostStatus["servicingState"].(string)
	if servicingState != expectedState {
		tc.Fail(fmt.Sprintf("expected servicingState %q, got %q",
			expectedState, servicingState))
		return nil
	}
	logrus.Infof("servicingState matches expected: %s", expectedState)

	// Validate lastError contains "Failed health check(s)"
	lastError, ok := hostStatus["lastError"]
	if !ok {
		tc.Fail("lastError not found in host status")
		return nil
	}

	lastErrorYAML, err := yaml.Marshal(lastError)
	if err != nil {
		return fmt.Errorf("failed to serialize lastError to YAML: %w", err)
	}

	if !strings.Contains(string(lastErrorYAML), "Failed health check(s)") {
		tc.Fail(fmt.Sprintf("lastError does not contain 'Failed health check(s)': %s",
			string(lastErrorYAML)))
		return nil
	}
	logrus.Info("lastError contains expected health check failure message")

	// Validate abActiveVolume
	if expectedState == "not-provisioned" {
		// When not provisioned, abActiveVolume should not be set
		if _, exists := hostStatus["abActiveVolume"]; exists {
			tc.Fail("abActiveVolume should not be set when state is not-provisioned")
			return nil
		}
		logrus.Info("abActiveVolume correctly absent for not-provisioned state")
	} else {
		// After A/B update rollback, active volume should remain unchanged
		hsActiveVolume, _ := hostStatus["abActiveVolume"].(string)
		expectedVolume := "volume-a"
		if hsActiveVolume != expectedVolume {
			tc.Fail(fmt.Sprintf("expected abActiveVolume %q, got %q",
				expectedVolume, hsActiveVolume))
			return nil
		}
		logrus.Infof("abActiveVolume matches expected: %s", expectedVolume)
	}

	// Validate health check failure log files
	listLogsOut, err := sudoCommand(s.sshClient,
		"ls /var/lib/trident/trident-health-check-failure-*.log")
	if err != nil {
		tc.Fail(fmt.Sprintf("failed to list health check failure logs: %v", err))
		return nil
	}

	logFiles := strings.Split(strings.TrimSpace(listLogsOut), "\n")
	if len(logFiles) != 1 {
		tc.Fail(fmt.Sprintf("expected exactly 1 health check failure log file, found %d",
			len(logFiles)))
		return nil
	}
	logrus.Infof("Found health check failure log: %s", logFiles[0])

	// Read log file contents
	logContent, err := sudoCommand(s.sshClient, fmt.Sprintf("cat %s", logFiles[0]))
	if err != nil {
		tc.Fail(fmt.Sprintf("failed to read health check failure log: %v", err))
		return nil
	}

	// Verify expected failure messages in log
	expectedMessages := []string{
		"Script 'invoke-rollback-from-script' failed",
		"Unit non-existent-service1.service could not be found",
		"Unit non-existent-service2.service could not be found",
	}

	for _, msg := range expectedMessages {
		if !strings.Contains(logContent, msg) {
			tc.Fail(fmt.Sprintf("health check failure log does not contain %q", msg))
			return nil
		}
	}

	logrus.Info("Rollback validation passed")
	return nil
}
