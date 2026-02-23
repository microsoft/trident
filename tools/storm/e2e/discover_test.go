package e2e

import (
	"testing"

	"github.com/sirupsen/logrus"
)

func TestGetTestSelectionPath(t *testing.T) {
	got := getTestSelectionPath("base")
	expected := "configurations/trident_configurations/base/test-selection.yaml"
	if got != expected {
		t.Fatalf("expected %q, got %q", expected, got)
	}
}

func TestGetTestSelectionPath_Hyphenated(t *testing.T) {
	got := getTestSelectionPath("encrypted-partition")
	expected := "configurations/trident_configurations/encrypted-partition/test-selection.yaml"
	if got != expected {
		t.Fatalf("expected %q, got %q", expected, got)
	}
}

// TestDiscoverTridentScenarios_TestTagsPresent verifies that discovered
// scenarios contain test tags from their test-selection.yaml. This exercises
// the full discovery path including embedded file reading.
func TestDiscoverTridentScenarios_TestTagsPresent(t *testing.T) {
	log := logrus.New()
	scenarios, err := DiscoverTridentScenarios(log)
	if err != nil {
		t.Fatalf("DiscoverTridentScenarios failed: %v", err)
	}

	if len(scenarios) == 0 {
		t.Fatal("expected at least one scenario")
	}

	// Every scenario should have at least one test tag.
	for _, s := range scenarios {
		testTags := s.TestTags()
		if len(testTags) == 0 {
			t.Errorf("scenario %q has no test tags", s.Name())
		}

		// All test tags should have the "test:" prefix.
		for _, tag := range testTags {
			if len(tag) <= len(TestTagPrefix) || tag[:len(TestTagPrefix)] != TestTagPrefix {
				t.Errorf("scenario %q: test tag %q missing %q prefix", s.Name(), tag, TestTagPrefix)
			}
		}

		// Test tags should also be present in the scenario's Tags() list.
		allTags := s.Tags()
		tagSet := make(map[string]bool)
		for _, tag := range allTags {
			tagSet[tag] = true
		}
		for _, testTag := range testTags {
			if !tagSet[testTag] {
				t.Errorf("scenario %q: test tag %q not found in Tags() list", s.Name(), testTag)
			}
		}
	}
}

// TestDiscoverTridentScenarios_KnownConfigTestTags checks specific
// configurations have expected test tags.
func TestDiscoverTridentScenarios_KnownConfigTestTags(t *testing.T) {
	log := logrus.New()
	scenarios, err := DiscoverTridentScenarios(log)
	if err != nil {
		t.Fatalf("DiscoverTridentScenarios failed: %v", err)
	}

	// Build a map of scenario name → test tags for lookup.
	scenarioMap := make(map[string][]string)
	for _, s := range scenarios {
		scenarioMap[s.Name()] = s.TestTags()
	}

	// The "base" config is always allowed. Verify its test tags.
	baseTags, ok := scenarioMap["base_vm-host"]
	if !ok {
		t.Fatal("expected base_vm-host scenario to be discovered")
	}

	if len(baseTags) != 1 || baseTags[0] != "test:base" {
		t.Errorf("base_vm-host: expected [test:base], got %v", baseTags)
	}

	// Verify that HasTestTag works through the scenario accessor.
	for _, s := range scenarios {
		if s.Name() == "base_vm-host" {
			if !s.HasTestTag("test:base") {
				t.Error("base_vm-host: HasTestTag('test:base') should be true")
			}
			if s.HasTestTag("test:encryption") {
				t.Error("base_vm-host: HasTestTag('test:encryption') should be false")
			}
			break
		}
	}
}
