package e2e

import (
	"encoding/json"
	"fmt"
	"strings"
	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils"

	"github.com/microsoft/storm/pkg/storm/core"
	"github.com/sirupsen/logrus"
)

type TridentE2EMatrixScriptSet struct {
	E2eScenarioMatrix TridentE2EScenarioMatrix `cmd:"" name:"e2e-matrix" help:"Generate a DevOps matrix for Trident E2E scenarios"`
}

type TridentE2EScenarioMatrix struct {
	TestRing testrings.TestRing `arg:"" name:"test-ring" help:"The current test ring to consider when generating the matrix"`
}

func (s *TridentE2EScenarioMatrix) Run(suite core.SuiteContext) error {
	for _, hw := range scenario.HardwareTypes() {
		for _, rt := range scenario.RuntimeTypes() {
			matchingScenarios := GetScenariosByHardwareAndRuntime(suite, hw, rt, s.TestRing)
			matrixJson, err := s.GenerateMatrix(matchingScenarios, hw, rt)
			if err != nil {
				return fmt.Errorf("failed to generate matrix for hardware '%s' and runtime '%s': %w", hw, rt, err)
			}
			variable := fmt.Sprintf("TEST_MATRIX_E2E_%s_%s", strings.ToUpper(hw.ToString()), strings.ToUpper(rt.ToString()))
			utils.SetAzureDevopsVariables(variable, matrixJson)
			logrus.Infof("Generated matrix for hardware '%s' and runtime '%s' with %d scenarios: %s", hw, rt, len(matchingScenarios), variable)
		}
	}

	return nil
}

func GetScenariosByHardwareAndRuntime(suite core.SuiteContext, hardware scenario.HardwareType, runtime scenario.RuntimeType, testRing testrings.TestRing) []string {
	scenarios := suite.Scenarios()
	outputScenarios := []string{}
	for _, sc := range scenarios {
		// Only consider Trident E2E scenarios
		tridentScenario, ok := sc.(*scenario.TridentE2EScenario)
		if !ok {
			continue
		}

		if !tridentScenario.TestRings().Contains(testRing) {
			continue
		}

		if tridentScenario.HardwareType() == hardware && tridentScenario.RuntimeType() == runtime {
			outputScenarios = append(outputScenarios, tridentScenario.Name())
		}
	}

	return outputScenarios
}

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
