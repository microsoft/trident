package validate

import (
	"strings"
	"testing"

	"github.com/Jeffail/gabs/v2"

	tridentutil "tridenttools/storm/utils/trident"
)

// The fixtures below use the real schema field, `health.checks` - the name
// Trident emits and the checked-in configurations use. An earlier fixture said
// `healthChecks`, which is not a schema field: gabs returned nil for the absent
// `checks` path, so the test passed through the no-checks fallback and never
// exercised a real check list.
func TestHasRollbackIntent(t *testing.T) {
	for name, tc := range map[string]struct {
		spec string
		want bool
	}{
		"failing check present": {
			spec: "spec:\n  health:\n    checks:\n    - name: invoke-rollback-from-script\n      runOn: [clean-install]\n",
			want: true,
		},
		"no health section": {
			spec: "spec:\n  storage: {}\n",
			want: false,
		},
		"health section with no checks": {
			// Preserve the original broader signal rather than assume no intent.
			spec: "spec:\n  health: {}\n",
			want: true,
		},
		"explicitly null checks": {
			spec: "spec:\n  health:\n    checks: null\n",
			want: true,
		},
		"only the injected UEFI checks": {
			// These are expected to PASS, so they must not imply intent.
			spec: "spec:\n  health:\n    checks:\n" +
				"    - name: " + UefiFallbackInstallCheckName + "\n" +
				"    - name: " + UefiFallbackAbUpdateCheckName + "\n",
			want: false,
		},
		"UEFI checks alongside a failing one": {
			spec: "spec:\n  health:\n    checks:\n" +
				"    - name: " + UefiFallbackInstallCheckName + "\n" +
				"    - name: invoke-rollback-from-script\n",
			want: true,
		},
		"checks emptied by the rollback cleanup": {
			spec: "spec:\n  health:\n    checks: []\n",
			want: false,
		},
	} {
		t.Run(name, func(t *testing.T) {
			hs, err := tridentutil.NewHostStatusFromYaml([]byte(tc.spec))
			if err != nil {
				t.Fatalf("parse host status: %v", err)
			}
			if got := HasRollbackIntent(hs); got != tc.want {
				t.Errorf("HasRollbackIntent = %v, want %v", got, tc.want)
			}
		})
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

// SecondaryGroups must read the schema field name. Reading `groups` (as both
// this validator and the legacy pytest did) silently matches nothing, so the
// membership assertion never runs.
func TestSecondaryGroupsReadsTheSchemaField(t *testing.T) {
	user, err := gabs.ParseJSON([]byte(`{
		"name": "testing-user",
		"secondaryGroups": ["wheel", "docker"],
		"groups": ["should-be-ignored"]
	}`))
	if err != nil {
		t.Fatalf("parse user: %v", err)
	}

	got := SecondaryGroups(user)
	if len(got) != 2 || got[0] != "wheel" || got[1] != "docker" {
		t.Errorf("SecondaryGroups = %v, want [wheel docker]", got)
	}

	noGroups, _ := gabs.ParseJSON([]byte(`{"name": "u"}`))
	if got := SecondaryGroups(noGroups); len(got) != 0 {
		t.Errorf("expected no groups for a user that declares none, got %v", got)
	}

	// Non-string entries are skipped rather than panicking.
	mixed, _ := gabs.ParseJSON([]byte(`{"name":"u","secondaryGroups":["wheel",42,null]}`))
	if got := SecondaryGroups(mixed); len(got) != 1 || got[0] != "wheel" {
		t.Errorf("expected only the string entry, got %v", got)
	}
}
