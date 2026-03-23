package e2e

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm/pkg/storm/core"
	"github.com/sirupsen/logrus"
)

// testSuiteContext is a minimal implementation of core.SuiteContext for tests.
type testSuiteContext struct {
	scenarios []core.Scenario
	log       *logrus.Logger
}

func (t *testSuiteContext) Name() string                { return "test" }
func (t *testSuiteContext) Logger() *logrus.Logger       { return t.log }
func (t *testSuiteContext) Scenarios() []core.Scenario   { return t.scenarios }
func (t *testSuiteContext) Helpers() []core.Helper       { return nil }
func (t *testSuiteContext) AzureDevops() bool            { return false }
func (t *testSuiteContext) Context() context.Context     { return context.Background() }
func (t *testSuiteContext) Scenario(name string) core.Scenario {
	for _, s := range t.scenarios {
		if s.Name() == name {
			return s
		}
	}
	return nil
}
func (t *testSuiteContext) Helper(name string) core.Helper { return nil }

// TestPipelineMatrix_PRE2ERingProducesVMHostScenarios validates that the
// pr-e2e test ring generates a non-empty matrix for VM/HOST — the only
// hardware/runtime combination currently enabled in the ADO pipeline
// (storm_e2e.yml).
func TestPipelineMatrix_PRE2ERingProducesVMHostScenarios(t *testing.T) {
	suite := newTestSuiteContext(t)
	scenarios := GetScenariosByHardwareAndRuntime(suite, scenario.HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingPrE2e)

	if len(scenarios) == 0 {
		t.Fatal("pr-e2e ring must produce at least one VM/HOST scenario")
	}

	// PR-E2E ring should have a meaningful subset for quick validation.
	if len(scenarios) < 5 {
		t.Errorf("pr-e2e ring has only %d VM/HOST scenarios; expected at least 5", len(scenarios))
	}
}

// TestPipelineMatrix_GenerateMatrixFormat validates that the matrix JSON
// produced by GenerateMatrix matches the format expected by the ADO pipeline
// template (test_execution_template.yml). Each entry must have uppercase
// SCENARIO, HARDWARE, RUNTIME, and TEST_RING fields.
func TestPipelineMatrix_GenerateMatrixFormat(t *testing.T) {
	suite := newTestSuiteContext(t)
	scenarios := GetScenariosByHardwareAndRuntime(suite, scenario.HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingPrE2e)
	if len(scenarios) == 0 {
		t.Fatal("no scenarios to test matrix generation")
	}

	gen := &TridentE2EScenarioMatrix{TestRing: testrings.TestRingPrE2e}
	matrixJSON, err := gen.GenerateMatrix(scenarios, scenario.HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingPrE2e)
	if err != nil {
		t.Fatalf("GenerateMatrix failed: %v", err)
	}

	// Parse the JSON into a generic map to validate structure.
	var matrix map[string]map[string]string
	if err := json.Unmarshal([]byte(matrixJSON), &matrix); err != nil {
		t.Fatalf("matrix JSON is not valid: %v", err)
	}

	requiredFields := []string{"SCENARIO", "HARDWARE", "RUNTIME", "TEST_RING"}
	for name, entry := range matrix {
		for _, field := range requiredFields {
			val, ok := entry[field]
			if !ok || val == "" {
				t.Errorf("matrix entry %q missing required field %q", name, field)
			}
		}
		// The key should match the SCENARIO field value.
		if entry["SCENARIO"] != name {
			t.Errorf("matrix key %q does not match SCENARIO field %q", name, entry["SCENARIO"])
		}
	}
}

// TestPipelineMatrix_VariableNamingConvention validates that the ADO output
// variable name format (TEST_MATRIX_E2E_{HW}_{RT}) matches across the matrix
// script and the pipeline YAML templates.
func TestPipelineMatrix_VariableNamingConvention(t *testing.T) {
	for _, hw := range scenario.HardwareTypes() {
		for _, rt := range trident.RuntimeTypes() {
			varName := fmt.Sprintf("TEST_MATRIX_E2E_%s_%s",
				strings.ToUpper(hw.ToString()),
				strings.ToUpper(rt.ToString()))

			// Variable name must only contain uppercase letters, digits, and underscores
			// to be a valid ADO output variable.
			for _, ch := range varName {
				if !((ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || ch == '_') {
					t.Errorf("variable name %q contains invalid character %q", varName, string(ch))
				}
			}

			// Must start with the expected prefix.
			if !strings.HasPrefix(varName, "TEST_MATRIX_E2E_") {
				t.Errorf("variable name %q missing expected prefix", varName)
			}
		}
	}
}

// TestPipelineMatrix_FullValidationCoversAll18VMHost validates that the
// full-validation ring generates all 18 VM/HOST scenarios needed by the
// pipeline.
func TestPipelineMatrix_FullValidationCoversAll18VMHost(t *testing.T) {
	suite := newTestSuiteContext(t)
	scenarios := GetScenariosByHardwareAndRuntime(suite, scenario.HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingFullValidation)

	if len(scenarios) != 18 {
		t.Errorf("full-validation ring should produce exactly 18 VM/HOST scenarios, got %d: %v",
			len(scenarios), scenarios)
	}
}

// newTestSuiteContext creates a minimal suite context for tests that need
// to call GetScenariosByHardwareAndRuntime.
func newTestSuiteContext(t *testing.T) *testSuiteContext {
	t.Helper()
	log := logrus.New()
	discovered, err := DiscoverTridentScenarios(log)
	if err != nil {
		t.Fatalf("DiscoverTridentScenarios failed: %v", err)
	}
	// Convert to []core.Scenario
	coreScenarios := make([]core.Scenario, len(discovered))
	for i := range discovered {
		coreScenarios[i] = &discovered[i]
	}
	return &testSuiteContext{scenarios: coreScenarios, log: log}
}
