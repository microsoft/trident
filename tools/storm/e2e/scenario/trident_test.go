package scenario

import (
	"testing"
	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
)

// mockRegistrar records the names of test cases registered via RegisterTestCase.
type mockRegistrar struct {
	names []string
}

func (m *mockRegistrar) RegisterTestCase(name string, _ storm.TestCaseFunction) {
	m.names = append(m.names, name)
}

func (m *mockRegistrar) has(name string) bool {
	for _, n := range m.names {
		if n == name {
			return true
		}
	}
	return false
}

func newTestScenario(t *testing.T, testTags []string) *TridentE2EScenario {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(`os: {}`))
	if err != nil {
		t.Fatalf("failed to create host config: %v", err)
	}
	s, err := NewTridentE2EScenario(
		"test-scenario_vm-host",
		[]string{"e2e", "vm", "host", "test:base"},
		hc,
		TridentE2EHostConfigParams{},
		HardwareTypeVM,
		trident.RuntimeTypeHost,
		testrings.TestRingSet{testrings.TestRingPre},
		testTags,
	)
	if err != nil {
		t.Fatalf("failed to create scenario: %v", err)
	}
	return s
}

func TestTridentE2EScenario_TestTags(t *testing.T) {
	tags := []string{"test:base", "test:encryption"}
	s := newTestScenario(t, tags)

	got := s.TestTags()
	if len(got) != 2 {
		t.Fatalf("expected 2 test tags, got %d", len(got))
	}
	if got[0] != "test:base" || got[1] != "test:encryption" {
		t.Errorf("expected [test:base test:encryption], got %v", got)
	}
}

func TestTridentE2EScenario_TestTagsEmpty(t *testing.T) {
	s := newTestScenario(t, nil)
	got := s.TestTags()
	if len(got) != 0 {
		t.Fatalf("expected empty test tags, got %v", got)
	}
}

func TestTridentE2EScenario_HasTestTag(t *testing.T) {
	tags := []string{"test:base", "test:encryption", "test:verity"}
	s := newTestScenario(t, tags)

	if !s.HasTestTag("test:base") {
		t.Error("expected HasTestTag('test:base') to be true")
	}
	if !s.HasTestTag("test:encryption") {
		t.Error("expected HasTestTag('test:encryption') to be true")
	}
	if !s.HasTestTag("test:verity") {
		t.Error("expected HasTestTag('test:verity') to be true")
	}
	if s.HasTestTag("test:rollback") {
		t.Error("expected HasTestTag('test:rollback') to be false")
	}
	if s.HasTestTag("base") {
		t.Error("expected HasTestTag('base') without prefix to be false")
	}
	if s.HasTestTag("") {
		t.Error("expected HasTestTag('') to be false")
	}
}

func TestTridentE2EScenario_TagsIncludeTestTags(t *testing.T) {
	testTags := []string{"test:base", "test:encryption"}
	hc, _ := hostconfig.NewHostConfigFromYaml([]byte(`os: {}`))
	s, err := NewTridentE2EScenario(
		"combined_vm-host",
		[]string{"e2e", "vm", "host", "pre", "test:base", "test:encryption"},
		hc,
		TridentE2EHostConfigParams{},
		HardwareTypeVM,
		trident.RuntimeTypeHost,
		testrings.TestRingSet{testrings.TestRingPre},
		testTags,
	)
	if err != nil {
		t.Fatalf("failed to create scenario: %v", err)
	}

	// Verify that tags contain both scenario tags and test tags.
	allTags := s.Tags()
	tagSet := make(map[string]bool)
	for _, tag := range allTags {
		tagSet[tag] = true
	}

	if !tagSet["e2e"] {
		t.Error("expected 'e2e' in tags")
	}
	if !tagSet["test:base"] {
		t.Error("expected 'test:base' in tags")
	}
	if !tagSet["test:encryption"] {
		t.Error("expected 'test:encryption' in tags")
	}
}

// newTestScenarioWithConfig creates a scenario with the given YAML config and test tags.
func newTestScenarioWithConfig(t *testing.T, configYAML string, testTags []string) *TridentE2EScenario {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(configYAML))
	if err != nil {
		t.Fatalf("failed to create host config: %v", err)
	}
	s, err := NewTridentE2EScenario(
		"test-scenario_vm-host",
		[]string{"e2e", "vm", "host", "test:base"},
		hc,
		TridentE2EHostConfigParams{},
		HardwareTypeVM,
		trident.RuntimeTypeHost,
		testrings.TestRingSet{testrings.TestRingPre},
		testTags,
	)
	if err != nil {
		t.Fatalf("failed to create scenario: %v", err)
	}
	return s
}

// minimalAbUpdateConfig is a minimal host config with an abUpdate section.
const minimalAbUpdateConfig = `
os: {}
storage:
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
`

// TestRegisterTestCases_Phase3_Verity verifies that validate-verity is
// registered when either test:root_verity or test:usr_verity tag is present.
func TestRegisterTestCases_Phase3_Verity(t *testing.T) {
	tests := []struct {
		name     string
		tags     []string
		expectIt bool
	}{
		{"root_verity", []string{"test:base", "test:root_verity"}, true},
		{"usr_verity", []string{"test:base", "test:usr_verity"}, true},
		{"both_verity", []string{"test:base", "test:root_verity", "test:usr_verity"}, true},
		{"no_verity", []string{"test:base"}, false},
		{"encryption_only", []string{"test:base", "test:encryption"}, false},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			s := newTestScenarioWithConfig(t, `os: {}`, tc.tags)
			r := &mockRegistrar{}
			if err := s.RegisterTestCases(r); err != nil {
				t.Fatalf("RegisterTestCases failed: %v", err)
			}
			if got := r.has("validate-verity"); got != tc.expectIt {
				t.Errorf("validate-verity registered=%v, want %v", got, tc.expectIt)
			}
		})
	}
}

// TestRegisterTestCases_Phase3_Extensions verifies that validate-extensions is
// registered when test:extensions tag is present.
func TestRegisterTestCases_Phase3_Extensions(t *testing.T) {
	tests := []struct {
		name     string
		tags     []string
		expectIt bool
	}{
		{"with_extensions", []string{"test:base", "test:extensions"}, true},
		{"without_extensions", []string{"test:base"}, false},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			s := newTestScenarioWithConfig(t, `os: {}`, tc.tags)
			r := &mockRegistrar{}
			if err := s.RegisterTestCases(r); err != nil {
				t.Fatalf("RegisterTestCases failed: %v", err)
			}
			if got := r.has("validate-extensions"); got != tc.expectIt {
				t.Errorf("validate-extensions registered=%v, want %v", got, tc.expectIt)
			}
		})
	}
}

// TestRegisterTestCases_Phase3_Rollback verifies that validate-rollback is
// registered when test:rollback tag is present.
func TestRegisterTestCases_Phase3_Rollback(t *testing.T) {
	tests := []struct {
		name     string
		tags     []string
		expectIt bool
	}{
		{"with_rollback", []string{"test:rollback"}, true},
		{"without_rollback", []string{"test:base"}, false},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			s := newTestScenarioWithConfig(t, `os: {}`, tc.tags)
			r := &mockRegistrar{}
			if err := s.RegisterTestCases(r); err != nil {
				t.Fatalf("RegisterTestCases failed: %v", err)
			}
			if got := r.has("validate-rollback"); got != tc.expectIt {
				t.Errorf("validate-rollback registered=%v, want %v", got, tc.expectIt)
			}
		})
	}
}

// TestRegisterTestCases_Phase3_AbStaged verifies that the split AB update tests
// (including validate-staged) are registered when the config has an AB update section.
func TestRegisterTestCases_Phase3_AbStaged(t *testing.T) {
	tests := []struct {
		name       string
		configYAML string
		expectIt   bool
	}{
		{"with_ab_update", minimalAbUpdateConfig, true},
		{"without_ab_update", `os: {}`, false},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			s := newTestScenarioWithConfig(t, tc.configYAML, []string{"test:base"})
			r := &mockRegistrar{}
			if err := s.RegisterTestCases(r); err != nil {
				t.Fatalf("RegisterTestCases failed: %v", err)
			}
			if got := r.has("ab-update-split-validate-staged"); got != tc.expectIt {
				t.Errorf("ab-update-split-validate-staged registered=%v, want %v", got, tc.expectIt)
			}
		})
	}
}

// TestRegisterTestCases_AllPhase3_FeatureParity validates that for each of the
// 19 configuration profiles, the correct Phase 3 test cases are registered. This
// is the comprehensive feature-parity check against the original pytest test suite.
func TestRegisterTestCases_AllPhase3_FeatureParity(t *testing.T) {
	type configProfile struct {
		name       string
		configYAML string
		testTags   []string
		// Expected Phase 3 test registrations
		expectVerity     bool
		expectExtensions bool
		expectRollback   bool
		expectAbStaged   bool
	}

	noAB := `os: {}`

	profiles := []configProfile{
		{"base", minimalAbUpdateConfig, []string{"test:base"}, false, false, false, true},
		{"simple", noAB, []string{"test:base"}, false, false, false, false},
		{"combined", minimalAbUpdateConfig, []string{"test:base", "test:usr_verity", "test:encryption", "test:uki"}, true, false, false, true},
		{"encrypted-partition", noAB, []string{"test:base", "test:encryption"}, false, false, false, false},
		{"encrypted-raid", noAB, []string{"test:base", "test:encryption"}, false, false, false, false},
		{"encrypted-swap", noAB, []string{"test:base", "test:encryption"}, false, false, false, false},
		{"extensions", minimalAbUpdateConfig, []string{"test:base", "test:extensions"}, false, true, false, true},
		{"health-checks-install", noAB, []string{"test:rollback"}, false, false, true, false},
		{"memory-constraint-combined", minimalAbUpdateConfig, []string{"test:base", "test:usr_verity", "test:encryption", "test:uki"}, true, false, false, true},
		{"misc", minimalAbUpdateConfig, []string{"test:base"}, false, false, false, true},
		{"raid-big", noAB, []string{"test:base"}, false, false, false, false},
		{"raid-mirrored", minimalAbUpdateConfig, []string{"test:base"}, false, false, false, true},
		{"raid-resync-small", minimalAbUpdateConfig, []string{"test:base"}, false, false, false, true},
		{"raid-small", minimalAbUpdateConfig, []string{"test:base"}, false, false, false, true},
		{"rerun", minimalAbUpdateConfig, []string{"test:base", "test:usr_verity", "test:encryption", "test:uki"}, true, false, false, true},
		{"root-verity", minimalAbUpdateConfig, []string{"test:base", "test:root_verity", "test:extensions"}, true, true, false, true},
		{"split", noAB, []string{"test:base"}, false, false, false, false},
		{"usr-verity", minimalAbUpdateConfig, []string{"test:base", "test:usr_verity", "test:uki"}, true, false, false, true},
		{"usr-verity-raid", noAB, []string{"test:base", "test:usr_verity", "test:uki"}, true, false, false, false},
	}

	for _, p := range profiles {
		t.Run(p.name, func(t *testing.T) {
			s := newTestScenarioWithConfig(t, p.configYAML, p.testTags)
			r := &mockRegistrar{}
			if err := s.RegisterTestCases(r); err != nil {
				t.Fatalf("RegisterTestCases failed: %v", err)
			}

			check := func(testCaseName string, expected bool) {
				if got := r.has(testCaseName); got != expected {
					t.Errorf("%s registered=%v, want %v (registered: %v)", testCaseName, got, expected, r.names)
				}
			}

			check("validate-verity", p.expectVerity)
			check("validate-extensions", p.expectExtensions)
			check("validate-rollback", p.expectRollback)
			check("ab-update-split-validate-staged", p.expectAbStaged)
		})
	}
}
