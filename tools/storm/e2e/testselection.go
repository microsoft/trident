package e2e

import (
	"fmt"

	"gopkg.in/yaml.v3"
)

const (
	// TestTagPrefix is the prefix applied to all test selection markers when
	// converting them to storm scenario tags.
	TestTagPrefix = "test:"
)

// TestSelection represents the parsed contents of a test-selection.yaml file.
// The "compatible" list contains the base set of test markers that a
// configuration supports. Ring-level overrides can add or remove markers for
// specific pipeline stages.
type TestSelection struct {
	Compatible  []string               `yaml:"compatible"`
	Weekly      *TestSelectionOverride  `yaml:"weekly,omitempty"`
	Daily       *TestSelectionOverride  `yaml:"daily,omitempty"`
	PostMerge   *TestSelectionOverride  `yaml:"post_merge,omitempty"`
	PullRequest *TestSelectionOverride  `yaml:"pullrequest,omitempty"`
	Validation  *TestSelectionOverride  `yaml:"validation,omitempty"`
}

// TestSelectionOverride describes markers to add or remove relative to the
// compatible set for a specific pipeline ring.
type TestSelectionOverride struct {
	Add    []string `yaml:"add,omitempty"`
	Remove []string `yaml:"remove,omitempty"`
}

// ParseTestSelection parses a test-selection.yaml file into a TestSelection.
func ParseTestSelection(data []byte) (*TestSelection, error) {
	var ts TestSelection
	if err := yaml.Unmarshal(data, &ts); err != nil {
		return nil, fmt.Errorf("failed to parse test-selection YAML: %w", err)
	}
	return &ts, nil
}

// TestTags returns the compatible markers as storm scenario tags with the
// "test:" prefix. This is the base set of test tags for the configuration.
func (ts *TestSelection) TestTags() []string {
	tags := make([]string, 0, len(ts.Compatible))
	for _, marker := range ts.Compatible {
		tags = append(tags, TestTagPrefix+marker)
	}
	return tags
}

// TestTagsForRing returns the resolved set of test tags after applying any
// ring-specific overrides. If no override exists for the given ring, the base
// compatible tags are returned. The ring parameter should match one of the
// YAML keys: "weekly", "daily", "post_merge", "pullrequest", "validation".
func (ts *TestSelection) TestTagsForRing(ring string) []string {
	override := ts.overrideForRing(ring)
	if override == nil {
		return ts.TestTags()
	}
	return applyOverride(ts.Compatible, override)
}

// overrideForRing returns the TestSelectionOverride for a given ring name, or
// nil if no override is defined.
func (ts *TestSelection) overrideForRing(ring string) *TestSelectionOverride {
	switch ring {
	case "weekly":
		return ts.Weekly
	case "daily":
		return ts.Daily
	case "post_merge":
		return ts.PostMerge
	case "pullrequest":
		return ts.PullRequest
	case "validation":
		return ts.Validation
	default:
		return nil
	}
}

// RingNames returns the list of recognised ring override names.
func RingNames() []string {
	return []string{"weekly", "daily", "post_merge", "pullrequest", "validation"}
}

// applyOverride computes the resolved marker list by starting from the base
// compatible set, removing any markers in override.Remove, then appending any
// markers in override.Add. The result is returned as prefixed tags.
func applyOverride(compatible []string, override *TestSelectionOverride) []string {
	// Build a set from the compatible markers.
	markerSet := make(map[string]struct{}, len(compatible))
	for _, m := range compatible {
		markerSet[m] = struct{}{}
	}

	// Remove entries.
	for _, m := range override.Remove {
		delete(markerSet, m)
	}

	// Add entries.
	for _, m := range override.Add {
		markerSet[m] = struct{}{}
	}

	// Convert to sorted tag list for deterministic output.
	tags := make([]string, 0, len(markerSet))
	// Preserve order: first compatible (if still present), then added.
	seen := make(map[string]bool, len(markerSet))
	for _, m := range compatible {
		if _, ok := markerSet[m]; ok && !seen[m] {
			tags = append(tags, TestTagPrefix+m)
			seen[m] = true
		}
	}
	for _, m := range override.Add {
		if _, ok := markerSet[m]; ok && !seen[m] {
			tags = append(tags, TestTagPrefix+m)
			seen[m] = true
		}
	}

	return tags
}

// HasMarker reports whether the compatible list contains the given marker.
func (ts *TestSelection) HasMarker(marker string) bool {
	for _, m := range ts.Compatible {
		if m == marker {
			return true
		}
	}
	return false
}
