package e2e

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils"

	"github.com/microsoft/storm/pkg/storm/core"
)

type TridentE2EMatrixScriptSet struct {
	E2eScenarioMatrix TridentE2EScenarioMatrix `cmd:"" name:"e2e-matrix" help:"Generate a DevOps matrix for Trident E2E scenarios"`
}

// This script receives a test ring as input and generates Azure DevOps matrix
// variables for all combinations of hardware and runtime types. Each matrix
// variable will contain the list of all scenarios that must be run for this
// ring with its combination of settings.
type TridentE2EScenarioMatrix struct {
	TestRing testrings.TestRing `arg:"" name:"test-ring" help:"The current test ring to consider when generating the matrix"`
}

func (s *TridentE2EScenarioMatrix) Run(suite core.SuiteContext) error {
	suite.Logger().Infof("Generating Trident E2E test matrices for test ring '%s'", s.TestRing.ToString())

	// Iterate over all hardware and runtime types to generate the corresponding matrices
	for _, hw := range scenario.HardwareTypes() {
		for _, rt := range scenario.RuntimeTypes() {

			// Get all matching scenarios for this hardware/runtime/testring combination
			matchingScenarios := GetScenariosByHardwareAndRuntime(suite, hw, rt, s.TestRing)
			slices.Sort(matchingScenarios)

			// Generate the matrix JSON
			matrixJson, err := s.GenerateMatrix(matchingScenarios, hw, rt)
			if err != nil {
				return fmt.Errorf("failed to generate matrix for hardware '%s' and runtime '%s': %w", hw, rt, err)
			}

			// Set the Azure DevOps output variable name to contain the hardware and runtime types
			variable := fmt.Sprintf("TEST_MATRIX_E2E_%s_%s", strings.ToUpper(hw.ToString()), strings.ToUpper(rt.ToString()))

			// Set the output variable
			utils.SetAzureDevopsOutputVariable(variable, matrixJson)

			// Log summary
			scenarioNames := ""
			for _, name := range matchingScenarios {
				scenarioNames += fmt.Sprintf(" - %s\n", name)
			}
			suite.Logger().Infof("Generated matrix for hardware '%s' and runtime '%s' with %d scenarios:\n%s", hw, rt, len(matchingScenarios), scenarioNames)
		}
	}

	return nil
}

// This function returns all e2e test scenarios that match the given hardware type,
// runtime type, and are enabled at the provided test ring.
func GetScenariosByHardwareAndRuntime(suite core.SuiteContext, hardware scenario.HardwareType, runtime scenario.RuntimeType, testRing testrings.TestRing) []string {
	// Get all scenarios from the suite
	scenarios := suite.Scenarios()
	outputScenarios := []string{}
	for _, sc := range scenarios {
		// Only consider Trident E2E scenarios
		tridentScenario, ok := sc.(*scenario.TridentE2EScenario)
		if !ok {
			continue
		}

		// Check if the scenario is enabled for the given test ring
		if !tridentScenario.TestRings().Contains(testRing) {
			continue
		}

		// Check if the scenario matches the given hardware and runtime types
		if tridentScenario.HardwareType() != hardware || tridentScenario.RuntimeType() != runtime {
			continue
		}

		// Add the scenario name to the output list
		outputScenarios = append(outputScenarios, tridentScenario.Name())
	}

	return outputScenarios
}

// Receives a list of scenario names and generates a JSON matrix containing
// those scenarios along with the provided hardware and runtime types.
//
// Example:
//
// ```json
//
//	{
//	  "scenario1": {
//	    "scenario": "scenario1",
//	    "hardware": "baremetal",
//	    "runtime": "kubernetes"
//	  },
//	  "scenario2": {
//	    "scenario": "scenario2",
//	    "hardware": "baremetal",
//	    "runtime": "kubernetes"
//	  }
//	}
//
// ```
//
// Note: the example is indented for readability; the actual output is not indented.
func (s *TridentE2EScenarioMatrix) GenerateMatrix(matchingScenarios []string, hardware scenario.HardwareType, runtime scenario.RuntimeType) (string, error) {
	output := make(outputMatrix)
	for _, scenarioName := range matchingScenarios {
		entry := matrixEntry{
			Scenario: scenarioName,
			Hardware: string(hardware),
			Runtime:  string(runtime),
		}
		output[scenarioName] = entry
	}

	rawJson, err := json.Marshal(output)
	if err != nil {
		return "", fmt.Errorf("failed to marshal matrix to JSON: %w", err)
	}

	return string(rawJson), nil
}

type outputMatrix map[string]matrixEntry

type matrixEntry struct {
	Scenario string `json:"scenario"`
	Hardware string `json:"hardware"`
	Runtime  string `json:"runtime"`
}
