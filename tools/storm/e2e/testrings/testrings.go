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

// Return the order index of the test ring. Lower index means lower ring. The
// number does not mean anything outside of comparison between rings.
func (tr TestRing) Order() uint {
	for i, ring := range pipelineRingsOrder {
		if ring == tr {
			return uint(i)
		}
	}

	// Unknown rings are considered highest order
	return uint(len(pipelineRingsOrder))
}

func (tr TestRing) Compare(other TestRing) int {
	thisOrder := tr.Order()
	otherOrder := other.Order()

	if thisOrder < otherOrder {
		return -1
	} else if thisOrder > otherOrder {
		return 1
	} else {
		return 0
	}
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

// Container for a set of test rings.
type TestRingSet []TestRing

// Contains reports whether the test ring set contains the specified ring.
func (trs TestRingSet) Contains(ring TestRing) bool {
	return slices.Contains(trs, ring)
}

// Lowest returns the lowest test ring in the set.
func (trs TestRingSet) Lowest() (TestRing, error) {
	if len(trs) == 0 {
		return TestRingEmpty, fmt.Errorf("test ring set is empty")
	}

	lowest := slices.MinFunc(trs, func(a, b TestRing) int {
		return a.Compare(b)
	})

	return lowest, nil
}
