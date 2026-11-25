package e2e

import (
	"encoding/json"
	"testing"

	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"

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
