package e2e

import (
	"reflect"
	"testing"
)

func TestParseTestSelection_SimpleCompatible(t *testing.T) {
	input := []byte(`compatible:
  - base
`)
	ts, err := ParseTestSelection(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ts.Compatible) != 1 || ts.Compatible[0] != "base" {
		t.Fatalf("expected [base], got %v", ts.Compatible)
	}
	if ts.Weekly != nil || ts.Daily != nil || ts.PostMerge != nil ||
		ts.PullRequest != nil || ts.Validation != nil {
		t.Error("expected all overrides to be nil")
	}
}

func TestParseTestSelection_MultipleMarkers(t *testing.T) {
	input := []byte(`compatible:
  - base
  - usr_verity
  - encryption
  - uki
`)
	ts, err := ParseTestSelection(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	expected := []string{"base", "usr_verity", "encryption", "uki"}
	if !reflect.DeepEqual(ts.Compatible, expected) {
		t.Fatalf("expected %v, got %v", expected, ts.Compatible)
	}
}

func TestParseTestSelection_WithOverrides(t *testing.T) {
	input := []byte(`compatible:
  - marker1
  - marker2
  - marker3
  - marker4
  - marker5
weekly:
  remove:
  - marker5
daily:
  remove:
  - marker5
post_merge:
  remove:
  - marker4
  - marker2
  add:
  - extra_test
pullrequest:
  remove:
  - marker3
validation:
  remove:
  - marker1
  add:
  - val_test1
  - val_test2
`)
	ts, err := ParseTestSelection(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(ts.Compatible) != 5 {
		t.Fatalf("expected 5 compatible markers, got %d", len(ts.Compatible))
	}

	// Weekly override
	if ts.Weekly == nil {
		t.Fatal("expected weekly override")
	}
	if len(ts.Weekly.Remove) != 1 || ts.Weekly.Remove[0] != "marker5" {
		t.Errorf("weekly remove: expected [marker5], got %v", ts.Weekly.Remove)
	}

	// Post-merge override
	if ts.PostMerge == nil {
		t.Fatal("expected post_merge override")
	}
	if len(ts.PostMerge.Remove) != 2 {
		t.Errorf("post_merge remove: expected 2 items, got %d", len(ts.PostMerge.Remove))
	}
	if len(ts.PostMerge.Add) != 1 || ts.PostMerge.Add[0] != "extra_test" {
		t.Errorf("post_merge add: expected [extra_test], got %v", ts.PostMerge.Add)
	}

	// Validation override
	if ts.Validation == nil {
		t.Fatal("expected validation override")
	}
	if len(ts.Validation.Add) != 2 {
		t.Errorf("validation add: expected 2 items, got %d", len(ts.Validation.Add))
	}
}

func TestTestTags_SimpleCompatible(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base", "encryption"},
	}
	tags := ts.TestTags()
	expected := []string{"test:base", "test:encryption"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTags_EmptyCompatible(t *testing.T) {
	ts := &TestSelection{}
	tags := ts.TestTags()
	if len(tags) != 0 {
		t.Fatalf("expected empty tags, got %v", tags)
	}
}

func TestTestTagsForRing_NoOverride(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base", "encryption"},
	}
	// No weekly override defined, should return base tags.
	tags := ts.TestTagsForRing("weekly")
	expected := []string{"test:base", "test:encryption"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTagsForRing_UnknownRing(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base"},
	}
	// Unknown ring should return base tags.
	tags := ts.TestTagsForRing("unknown_ring")
	expected := []string{"test:base"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTagsForRing_WithRemove(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"marker1", "marker2", "marker3"},
		PullRequest: &TestSelectionOverride{
			Remove: []string{"marker2"},
		},
	}
	tags := ts.TestTagsForRing("pullrequest")
	expected := []string{"test:marker1", "test:marker3"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTagsForRing_WithAdd(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base"},
		Validation: &TestSelectionOverride{
			Add: []string{"val_test1", "val_test2"},
		},
	}
	tags := ts.TestTagsForRing("validation")
	expected := []string{"test:base", "test:val_test1", "test:val_test2"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTagsForRing_WithAddAndRemove(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"marker1", "marker2", "marker3"},
		PostMerge: &TestSelectionOverride{
			Remove: []string{"marker2"},
			Add:    []string{"extra_test"},
		},
	}
	tags := ts.TestTagsForRing("post_merge")
	expected := []string{"test:marker1", "test:marker3", "test:extra_test"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTagsForRing_AllRings(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"a", "b", "c"},
		Weekly:      &TestSelectionOverride{Remove: []string{"c"}},
		Daily:       &TestSelectionOverride{Remove: []string{"b"}},
		PostMerge:   &TestSelectionOverride{Add: []string{"d"}},
		PullRequest: &TestSelectionOverride{Remove: []string{"a", "b"}},
		Validation:  &TestSelectionOverride{Remove: []string{"a"}, Add: []string{"e"}},
	}

	tests := []struct {
		ring     string
		expected []string
	}{
		{"weekly", []string{"test:a", "test:b"}},
		{"daily", []string{"test:a", "test:c"}},
		{"post_merge", []string{"test:a", "test:b", "test:c", "test:d"}},
		{"pullrequest", []string{"test:c"}},
		{"validation", []string{"test:b", "test:c", "test:e"}},
	}

	for _, tc := range tests {
		t.Run(tc.ring, func(t *testing.T) {
			tags := ts.TestTagsForRing(tc.ring)
			if !reflect.DeepEqual(tags, tc.expected) {
				t.Errorf("ring %s: expected %v, got %v", tc.ring, tc.expected, tags)
			}
		})
	}
}

func TestParseTestSelection_RealBaseConfig(t *testing.T) {
	// Matches the actual base/test-selection.yaml format.
	input := []byte(`compatible:
  - base
`)
	ts, err := ParseTestSelection(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	tags := ts.TestTags()
	if len(tags) != 1 || tags[0] != "test:base" {
		t.Fatalf("expected [test:base], got %v", tags)
	}
}

func TestParseTestSelection_RealCombinedConfig(t *testing.T) {
	// Matches the actual combined/test-selection.yaml format.
	input := []byte(`compatible:
  - base
  - usr_verity
  - encryption
  - uki
`)
	ts, err := ParseTestSelection(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	tags := ts.TestTags()
	expected := []string{"test:base", "test:usr_verity", "test:encryption", "test:uki"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestHasMarker(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base", "encryption"},
	}

	if !ts.HasMarker("base") {
		t.Error("expected HasMarker('base') to return true")
	}
	if !ts.HasMarker("encryption") {
		t.Error("expected HasMarker('encryption') to return true")
	}
	if ts.HasMarker("verity") {
		t.Error("expected HasMarker('verity') to return false")
	}
	if ts.HasMarker("") {
		t.Error("expected HasMarker('') to return false")
	}
}

func TestParseTestSelection_InvalidYAML(t *testing.T) {
	input := []byte(`{{{invalid yaml`)
	_, err := ParseTestSelection(input)
	if err == nil {
		t.Fatal("expected error for invalid YAML")
	}
}

func TestParseTestSelection_EmptyInput(t *testing.T) {
	input := []byte(``)
	ts, err := ParseTestSelection(input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ts.Compatible) != 0 {
		t.Fatalf("expected empty compatible, got %v", ts.Compatible)
	}
}

func TestRingNames(t *testing.T) {
	names := RingNames()
	expected := []string{"weekly", "daily", "post_merge", "pullrequest", "validation"}
	if !reflect.DeepEqual(names, expected) {
		t.Fatalf("expected %v, got %v", expected, names)
	}
}

func TestTestTagsForRing_RemoveNonexistent(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base", "encryption"},
		Weekly: &TestSelectionOverride{
			Remove: []string{"nonexistent"},
		},
	}
	tags := ts.TestTagsForRing("weekly")
	expected := []string{"test:base", "test:encryption"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v, got %v", expected, tags)
	}
}

func TestTestTagsForRing_AddDuplicate(t *testing.T) {
	ts := &TestSelection{
		Compatible: []string{"base"},
		Weekly: &TestSelectionOverride{
			Add: []string{"base"}, // already in compatible
		},
	}
	tags := ts.TestTagsForRing("weekly")
	expected := []string{"test:base"}
	if !reflect.DeepEqual(tags, expected) {
		t.Fatalf("expected %v (no duplicates), got %v", expected, tags)
	}
}
