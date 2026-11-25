package e2e

import (
	"encoding/json"
	"slices"
	"testing"

	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm/pkg/storm/core"
	"github.com/sirupsen/logrus"
)

func TestDiscoverAzl4HostScenario(t *testing.T) {
	scenarios, err := DiscoverTridentScenarios(logrus.New())
	if err != nil {
		t.Fatal(err)
	}
	count := 0
	for _, s := range scenarios {
		if s.Distro() != "azl4" {
			continue
		}
		count++
		if s.Name() != "base-azl4_vm-host" ||
			s.HardwareType() != scenario.HardwareTypeVM ||
			s.RuntimeType() != trident.RuntimeTypeHost {
			t.Fatalf("unexpected AZL4 scenario: %s", s.Name())
		}
		for _, ring := range []testrings.TestRing{
			testrings.TestRingPrE2e, testrings.TestRingCi,
			testrings.TestRingPre, testrings.TestRingFullValidation,
		} {
			if !s.TestRings().Contains(ring) {
				t.Errorf("AZL4 host scenario missing from %s", ring)
			}
		}
	}
	if count != 1 {
		t.Fatalf("expected one AZL4 host scenario, got %d", count)
	}
}

func TestAzl4MatrixCarriesDistroAndHostInstaller(t *testing.T) {
	matrix, err := (&TridentE2EScenarioMatrix{}).GenerateMatrix(
		[]string{"base-azl4_vm-host"}, scenario.HardwareTypeVM,
		trident.RuntimeTypeHost, testrings.TestRingPrE2e, "azl4",
	)
	if err != nil {
		t.Fatal(err)
	}
	var parsed outputMatrix
	if err := json.Unmarshal([]byte(matrix), &parsed); err != nil {
		t.Fatal(err)
	}
	entry := parsed["base-azl4_vm-host"]
	if entry.Distro != "azl4" || entry.ConfigurationName != "base-azl4" ||
		entry.InstallerIsoName != "trident-installer" ||
		entry.RuntimeEnvironment != "host" {
		t.Fatalf("incorrect AZL4 matrix routing: %+v", entry)
	}
}

type matrixTestSuite struct {
	core.SuiteContext
	scenarios []core.Scenario
}

func (s matrixTestSuite) Scenarios() []core.Scenario { return s.scenarios }

func TestMatrixSelectionDoesNotMixDistros(t *testing.T) {
	discovered, err := DiscoverTridentScenarios(logrus.New())
	if err != nil {
		t.Fatal(err)
	}
	suite := matrixTestSuite{}
	for i := range discovered {
		suite.scenarios = append(suite.scenarios, &discovered[i])
	}
	azl4 := GetScenariosByHardwareAndRuntime(suite, scenario.HardwareTypeVM,
		trident.RuntimeTypeHost, testrings.TestRingPrE2e, "azl4")
	if !slices.Equal(azl4, []string{"base-azl4_vm-host"}) {
		t.Fatalf("AZL4 matrix contains wrong scenarios: %v", azl4)
	}
	azl3 := GetScenariosByHardwareAndRuntime(suite, scenario.HardwareTypeVM,
		trident.RuntimeTypeHost, testrings.TestRingPrE2e, "azl3")
	if !slices.Contains(azl3, "base_vm-host") || slices.Contains(azl3, "base-azl4_vm-host") {
		t.Fatalf("legacy AZL3 matrix changed: %v", azl3)
	}
	unsupported := GetScenariosByHardwareAndRuntime(suite, scenario.HardwareTypeVM,
		trident.RuntimeTypeContainer, testrings.TestRingPrE2e, "azl4")
	if len(unsupported) != 0 {
		t.Fatalf("unsupported AZL4 container scenarios selected: %v", unsupported)
	}
}
