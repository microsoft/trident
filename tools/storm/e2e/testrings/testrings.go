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
		// 'none' and 'empty' are terminators of the ring order, not rings a
		// scenario can run in. Including them here would put them in every
		// scenario's target list, so asking for ring 'none' would select every
		// scenario instead of none of them.
		if found && !ring.IsNone() {
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

// DefaultTestRing is the ring used for a pipeline stage type that names no
// known ring. Falling back to CI-level validation keeps an unrecognised stage
// running a meaningful set of scenarios instead of silently producing an empty
// matrix and reporting success without testing anything.
const DefaultTestRing = TestRingCi

// pipelineStageAliases maps Azure DevOps stage types onto the ring they should
// run. A pipeline's stageType and a test ring are separate vocabularies that
// only happen to overlap on pr-e2e, ci and pre, and the pipeline templates pass
// the former straight through as the latter.
var pipelineStageAliases = map[TestRing]TestRing{
	"azl-validation": TestRingCi,
}

// Resolution describes how a stage type was resolved to a test ring.
type Resolution int

const (
	// ResolvedDirect means the value already named a test ring.
	ResolvedDirect Resolution = iota
	// ResolvedAlias means the value is a known stage type mapped to a ring.
	ResolvedAlias
	// ResolvedFallback means the value was not recognised and DefaultTestRing
	// was substituted.
	ResolvedFallback
)

// Resolve maps a pipeline stage type onto the test ring that should run. The
// returned Resolution lets callers report an alias or a fallback; 'none' and
// the empty ring resolve directly, so intentionally running nothing is
// preserved rather than being treated as unrecognised.
func Resolve(tr TestRing) (TestRing, Resolution) {
	if pipelineRingsOrder.Contains(tr) {
		return tr, ResolvedDirect
	}
	if mapped, ok := pipelineStageAliases[tr]; ok {
		return mapped, ResolvedAlias
	}
	return DefaultTestRing, ResolvedFallback
}
