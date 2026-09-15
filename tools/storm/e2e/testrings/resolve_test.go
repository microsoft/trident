package testrings

import "testing"

// A pipeline stage type that resolves to no ring produces an empty matrix, and
// the stage then reports success without running a single scenario. Every
// stage type the pipeline templates can pass must therefore land on a ring.
func TestResolveMapsStageTypesToRings(t *testing.T) {
	tests := []struct {
		in         TestRing
		want       TestRing
		resolution Resolution
	}{
		{TestRingPrE2e, TestRingPrE2e, ResolvedDirect},
		{TestRingCi, TestRingCi, ResolvedDirect},
		{TestRingPre, TestRingPre, ResolvedDirect},
		{TestRingFullValidation, TestRingFullValidation, ResolvedDirect},

		// Running nothing is a deliberate choice, not an unrecognised value.
		{TestRingNone, TestRingNone, ResolvedDirect},
		{TestRingEmpty, TestRingEmpty, ResolvedDirect},

		{"azl-validation", TestRingCi, ResolvedAlias},

		// Stage types with no mapping fall back rather than testing nothing.
		{"rel", DefaultTestRing, ResolvedFallback},
		{"pr-e2e-azure", DefaultTestRing, ResolvedFallback},
		{"nonsense", DefaultTestRing, ResolvedFallback},
	}

	for _, tt := range tests {
		t.Run(tt.in.ToString(), func(t *testing.T) {
			got, res := Resolve(tt.in)
			if got != tt.want || res != tt.resolution {
				t.Errorf("Resolve(%q) = (%q, %v), want (%q, %v)",
					tt.in, got, res, tt.want, tt.resolution)
			}
		})
	}
}

// The fallback only helps if it actually selects scenarios.
func TestDefaultTestRingIsARealRing(t *testing.T) {
	if DefaultTestRing.IsNone() {
		t.Fatalf("DefaultTestRing %q runs no scenarios", DefaultTestRing)
	}
	if !pipelineRingsOrder.Contains(DefaultTestRing) {
		t.Fatalf("DefaultTestRing %q is not a known ring", DefaultTestRing)
	}
}

// Aliases must point at rings, or they would reintroduce the empty matrix.
func TestAliasesResolveToKnownRings(t *testing.T) {
	for stage, ring := range pipelineStageAliases {
		if !pipelineRingsOrder.Contains(ring) {
			t.Errorf("alias %q maps to unknown ring %q", stage, ring)
		}
		if pipelineRingsOrder.Contains(stage) {
			t.Errorf("alias %q is already a ring; the alias is dead code", stage)
		}
	}
}
