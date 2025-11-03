package testrings

import "slices"

type TestRing string

const (
	TestRingPrE2e          TestRing = "pr-e2e"
	TestRingCi             TestRing = "ci"
	TestRingPre            TestRing = "pre"
	TestRingFullValidation TestRing = "full-validation"
)

var pipelineRingsOrder = TestRingSet{
	TestRingPrE2e,
	TestRingCi,
	TestRingPre,
	TestRingFullValidation,
}

func (tr TestRing) ToString() string {
	return string(tr)
}

func (tr TestRing) GetTargetList() TestRingSet {
	var targets []TestRing
	found := false
	for _, ring := range pipelineRingsOrder {
		if ring == tr {
			found = true
		}
		if found {
			targets = append(targets, ring)
		}
	}
	return targets
}

type TestRingSet []TestRing

func (trs TestRingSet) Contains(ring TestRing) bool {
	return slices.Contains(trs, ring)
}
