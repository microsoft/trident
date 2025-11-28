package testrings

import (
	"fmt"
	"slices"
)

type TestRing string

// Definition of known test rings.
const (
	TestRingEmpty          TestRing = ""
	TestRingNone           TestRing = "none"
	TestRingPrE2e          TestRing = "pr-e2e"
	TestRingCi             TestRing = "ci"
	TestRingPre            TestRing = "pre"
	TestRingFullValidation TestRing = "full-validation"
)

// Ordered list of test rings from lowest to highest.
var pipelineRingsOrder = TestRingSet{
	TestRingPrE2e,
	TestRingCi,
	TestRingPre,
	TestRingFullValidation,
	// The options below mean nothing should be run
	TestRingNone,
	TestRingEmpty,
}

func (tr *TestRing) UnmarshalYAML(unmarshal func(interface{}) error) error {
	var ringStr string
	if err := unmarshal(&ringStr); err != nil {
		return err
	}

	if !pipelineRingsOrder.Contains(TestRing(ringStr)) {
		return fmt.Errorf("unknown test ring: %s", ringStr)
	}

	*tr = TestRing(ringStr)
	return nil
}

func (tr TestRing) ToString() string {
	return string(tr)
}

func (tr TestRing) IsNone() bool {
	return tr == TestRingNone || tr == TestRingEmpty
}

// For a given test ring, return the list of this test ring and all "higher"
// rings in the pipeline order. If the ring is 'none' or 'empty', an empty list
// is returned.
func (tr TestRing) GetTargetList() (TestRingSet, error) {
	if tr.IsNone() {
		// On empty or 'none' ring, return an empty list
		return TestRingSet{}, nil
	}

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

	if !found {
		return nil, fmt.Errorf("unknown test ring: %s", tr)
	}

	return targets, nil
}

type TestRingSet []TestRing

func (trs TestRingSet) Contains(ring TestRing) bool {
	return slices.Contains(trs, ring)
}
