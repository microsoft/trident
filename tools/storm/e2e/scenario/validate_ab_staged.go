package scenario

import (
	"fmt"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/utils/trident"
)

// validateAbStaged validates that an A/B update has been staged correctly
// on the remote host. Converted from ab_update_staged_test.py test_ab_update_staged.
//
// It validates:
//   - servicingState is "ab-update-staged"
//   - abActiveVolume is unchanged from pre-update (volume-a)
func (s *TridentE2EScenario) validateAbStaged(tc storm.TestCase) error {
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

	// Validate servicingState is "ab-update-staged"
	servicingState, _ := hostStatus["servicingState"].(string)
	if servicingState != "ab-update-staged" {
		tc.Fail(fmt.Sprintf("expected servicingState %q, got %q",
			"ab-update-staged", servicingState))
		return nil
	}
	logrus.Info("servicingState matches expected: ab-update-staged")

	// Validate abActiveVolume is unchanged (should still be volume-a after staging)
	expectedVolume := "volume-a"
	hsActiveVolume, _ := hostStatus["abActiveVolume"].(string)
	if hsActiveVolume != expectedVolume {
		tc.Fail(fmt.Sprintf("expected abActiveVolume %q (unchanged from pre-update), got %q",
			expectedVolume, hsActiveVolume))
		return nil
	}
	logrus.Infof("abActiveVolume unchanged: %s", expectedVolume)

	logrus.Info("A/B update staged validation passed")
	return nil
}
