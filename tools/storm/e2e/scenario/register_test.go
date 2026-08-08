package scenario

import (
	"slices"
	"testing"

	"github.com/microsoft/storm/pkg/storm/core"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"
)

// fakeRegistrar records the order of registered test-case names.
type fakeRegistrar struct {
	names []string
}

func (f *fakeRegistrar) RegisterTestCase(name string, _ core.TestCaseFunction) {
	f.names = append(f.names, name)
}

func newScenarioForTest(t *testing.T, configYaml string) *TridentE2EScenario {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(configYaml))
	if err != nil {
		t.Fatalf("failed to parse config: %v", err)
	}
	s, err := NewTridentE2EScenario(
		"test", []string{"e2e"}, hc, TridentE2EHostConfigParams{},
		HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingSet{testrings.TestRingPrE2e},
	)
	if err != nil {
		t.Fatalf("failed to create scenario: %v", err)
	}
	return s
}

const abConfig = `
storage:
  abUpdate:
    volumePairs:
    - id: root
      volumeAId: root-a
      volumeBId: root-b
`

const noAbConfig = `
storage:
  disks:
  - id: os
    partitions:
    - id: root
      size: 8G
`

const raidConfig = `
storage:
  raid:
    software:
    - id: root
      name: root
      level: raid1
      devices: [root-a, root-b]
`

const usrVerityRaidConfig = `
storage:
  raid:
    software:
    - id: usr
      name: usr
      level: raid1
      devices: [usr-a, usr-b]
  verity:
  - id: usr
    name: usr
`

func TestRegisterTestCases_ABUpdate_RegistersValidation(t *testing.T) {
	s := newScenarioForTest(t, abConfig)
	var r fakeRegistrar
	if err := s.RegisterTestCases(&r); err != nil {
		t.Fatalf("RegisterTestCases error: %v", err)
	}

	// validate-install must come right after check-trident-ssh.
	assertOrder(t, r.names, "check-trident-ssh", "validate-install")
	// Image prep runs after prepare-hc and before setup-test-host.
	mustContain(t, r.names, "prepare-test-images")
	assertOrder(t, r.names, "prepare-hc", "prepare-test-images")
	assertOrder(t, r.names, "prepare-test-images", "setup-test-host")
	// Host diagnostics validation runs right after install validation.
	mustContain(t, r.names, "validate-host-diagnostics")
	assertOrder(t, r.names, "validate-install", "validate-host-diagnostics")
	// Post-A/B-update validations must be registered.
	mustContain(t, r.names, "validate-ab-update-1")
	mustContain(t, r.names, "validate-ab-update-split")
	// validate-ab-update-1 must come after the ab-update-1 update case.
	assertOrder(t, r.names, "ab-update-1-ab-update", "validate-ab-update-1")

	// Auto-rollback cases must be registered in order, after the first A/B
	// update's validation and before the split A/B update.
	for _, n := range []string{
		"auto-rollback-sync-hc", "auto-rollback-update-hc", "auto-rollback-inject-hc",
		"auto-rollback-upload-hc", "auto-rollback-update", "validate-auto-rollback",
	} {
		mustContain(t, r.names, n)
	}
	assertOrder(t, r.names, "validate-ab-update-1", "auto-rollback-sync-hc")
	assertOrder(t, r.names, "auto-rollback-update-hc", "auto-rollback-inject-hc")
	assertOrder(t, r.names, "auto-rollback-inject-hc", "auto-rollback-update")
	assertOrder(t, r.names, "auto-rollback-update", "validate-auto-rollback")
	assertOrder(t, r.names, "validate-auto-rollback", "ab-update-split-sync-hc")

	// Second A/B update (return into OS A) must be registered in order, after
	// the auto-rollback and before the split A/B update.
	for _, n := range []string{
		"ab-update-2-sync-hc", "ab-update-2-clear-hc", "ab-update-2-update-hc",
		"ab-update-2-upload-new-hc", "ab-update-2-ab-update", "validate-ab-update-2",
	} {
		mustContain(t, r.names, n)
	}
	assertOrder(t, r.names, "validate-auto-rollback", "ab-update-2-sync-hc")
	assertOrder(t, r.names, "ab-update-2-ab-update", "validate-ab-update-2")
	assertOrder(t, r.names, "validate-ab-update-2", "ab-update-split-sync-hc")

	// Manual rollback (VM A/B configs) must be registered after the split
	// validation, in order.
	mustContain(t, r.names, "manual-rollback")
	mustContain(t, r.names, "validate-manual-rollback")
	assertOrder(t, r.names, "validate-ab-update-split", "manual-rollback")
	assertOrder(t, r.names, "manual-rollback", "validate-manual-rollback")

	assertUnique(t, r.names)
}

func TestRegisterTestCases_NoABUpdate_OnlyInstallValidation(t *testing.T) {
	s := newScenarioForTest(t, noAbConfig)
	var r fakeRegistrar
	if err := s.RegisterTestCases(&r); err != nil {
		t.Fatalf("RegisterTestCases error: %v", err)
	}

	mustContain(t, r.names, "validate-install")
	if slices.Contains(r.names, "validate-ab-update-1") {
		t.Error("validate-ab-update-1 should not be registered without abUpdate")
	}
	if slices.Contains(r.names, "rebuild-raid") {
		t.Error("rebuild-raid should not be registered without RAID")
	}
	assertUnique(t, r.names)
}

func TestRegisterTestCases_Raid_RegistersRebuildRaid(t *testing.T) {
	s := newScenarioForTest(t, raidConfig)
	var r fakeRegistrar
	if err := s.RegisterTestCases(&r); err != nil {
		t.Fatalf("RegisterTestCases error: %v", err)
	}

	for _, n := range []string{"rebuild-raid-fail-disk", "rebuild-raid", "validate-rebuild-raid"} {
		mustContain(t, r.names, n)
	}
	assertOrder(t, r.names, "rebuild-raid-fail-disk", "rebuild-raid")
	assertOrder(t, r.names, "rebuild-raid", "validate-rebuild-raid")
	// A RAID config without abUpdate must not register A/B cases.
	if slices.Contains(r.names, "validate-ab-update-1") {
		t.Error("non-A/B RAID config should not register A/B update cases")
	}
	assertUnique(t, r.names)
}

func TestRegisterTestCases_UsrVerityRaid_NoRebuildRaid(t *testing.T) {
	s := newScenarioForTest(t, usrVerityRaidConfig)
	var r fakeRegistrar
	if err := s.RegisterTestCases(&r); err != nil {
		t.Fatalf("RegisterTestCases error: %v", err)
	}
	if slices.Contains(r.names, "rebuild-raid") {
		t.Error("usr-verity RAID config must not register rebuild-raid (verity rebuild unsupported)")
	}
}

func assertOrder(t *testing.T, names []string, before, after string) {
	t.Helper()
	bi := slices.Index(names, before)
	ai := slices.Index(names, after)
	if bi < 0 {
		t.Fatalf("%q not registered (have %v)", before, names)
	}
	if ai < 0 {
		t.Fatalf("%q not registered (have %v)", after, names)
	}
	if bi >= ai {
		t.Errorf("%q (idx %d) should come before %q (idx %d)", before, bi, after, ai)
	}
}

func mustContain(t *testing.T, names []string, want string) {
	t.Helper()
	if !slices.Contains(names, want) {
		t.Errorf("expected %q to be registered, have %v", want, names)
	}
}

func assertUnique(t *testing.T, names []string) {
	t.Helper()
	seen := map[string]struct{}{}
	for _, n := range names {
		if _, dup := seen[n]; dup {
			t.Errorf("duplicate test case name %q", n)
		}
		seen[n] = struct{}{}
	}
}
