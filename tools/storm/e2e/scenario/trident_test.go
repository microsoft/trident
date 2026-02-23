package scenario

import (
	"testing"
	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"
)

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
