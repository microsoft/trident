package collector

import (
	"fmt"
	"storm/pkg/storm/core"
)

type TestCaseMetadata struct {
	Name string
	F    core.TestCaseFunction
}

func CollectTestCases(r core.TestRegistrant) ([]TestCaseMetadata, error) {
	collector := testCaseCollector{
		testCases: make([]TestCaseMetadata, 0),
	}

	// Run the registration function to collect the test cases.
	err := r.RegisterTestCases(&collector)
	if err != nil {
		return nil, fmt.Errorf("failed to register test cases: %w", err)
	}

	// Check if names are valid and unique.
	names := make(map[string]bool)
	for _, testCase := range collector.testCases {
		if _, exists := names[testCase.Name]; exists {
			return nil, fmt.Errorf("test case name '%s' is not unique", testCase.Name)
		}

		err := core.ValidateEntityName(testCase.Name, "test case")
		if err != nil {
			return nil, err
		}

		names[testCase.Name] = true
	}

	return collector.testCases, nil
}

type testCaseCollector struct {
	testCases []TestCaseMetadata
}

// RegisterTestCase implements core.TestRegistrar.
func (c *testCaseCollector) RegisterTestCase(name string, f core.TestCaseFunction) {
	c.testCases = append(c.testCases, TestCaseMetadata{
		Name: name,
		F:    f,
	})
}
