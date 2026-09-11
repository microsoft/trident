package validate

import (
	"strings"
	"testing"

	tridentutil "tridenttools/storm/utils/trident"
)

func TestHasRollbackIntent(t *testing.T) {
	withHealth, _ := tridentutil.NewHostStatusFromYaml([]byte("spec:\n  health:\n    healthChecks: []\n"))
	if !HasRollbackIntent(withHealth) {
		t.Error("expected rollback intent when spec.health present")
	}
	withoutHealth, _ := tridentutil.NewHostStatusFromYaml([]byte("spec:\n  storage: {}\n"))
	if HasRollbackIntent(withoutHealth) {
		t.Error("did not expect rollback intent without spec.health")
	}
}

// TestRollbackHostStatusChecks exercises the host-status portion of the rollback
// contract (state, absent active volume, health-check lastError) that
// ValidateRollback asserts; the log-file portion requires SSH and is covered by
// integration runs.
func TestRollbackHostStatusChecks(t *testing.T) {
	hs, _ := tridentutil.NewHostStatusFromYaml([]byte(
		"servicingState: not-provisioned\nlastError:\n  message: Failed health check(s)\nspec:\n  health: {}\n"))

	if hs.ServicingState() != tridentutil.ServicingStateNotProvisioned {
		t.Errorf("state = %q, want not-provisioned", hs.ServicingState())
	}
	if _, present := hs.AbActiveVolume(); present {
		t.Error("abActiveVolume should be absent when not provisioned")
	}
	le, ok := hs.LastError()
	if !ok || !strings.Contains(le, rollbackFailedHealthError) {
		t.Errorf("lastError = %q, want it to contain %q", le, rollbackFailedHealthError)
	}
}

func TestValidateAbUpdateStaged(t *testing.T) {
	hs, _ := tridentutil.NewHostStatusFromYaml([]byte(
		"servicingState: ab-update-staged\nabActiveVolume: volume-a\n"))
	var sa SoftAsserter
	ValidateAbUpdateStaged(&sa, hs, tridentutil.AbVolumeA)
	if sa.HasFailures() {
		t.Errorf("expected no failures, got: %v", sa.Err())
	}

	// Wrong state -> failure.
	bad, _ := tridentutil.NewHostStatusFromYaml([]byte(
		"servicingState: provisioned\nabActiveVolume: volume-a\n"))
	var sa2 SoftAsserter
	ValidateAbUpdateStaged(&sa2, bad, tridentutil.AbVolumeA)
	if !sa2.HasFailures() {
		t.Error("expected failure for non-staged state")
	}

	// Volume already flipped -> failure.
	flipped, _ := tridentutil.NewHostStatusFromYaml([]byte(
		"servicingState: ab-update-staged\nabActiveVolume: volume-b\n"))
	var sa3 SoftAsserter
	ValidateAbUpdateStaged(&sa3, flipped, tridentutil.AbVolumeA)
	if !sa3.HasFailures() {
		t.Error("expected failure when active volume already changed")
	}
}
