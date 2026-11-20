package e2e

import (
	"encoding/json"
	"strings"
	"testing"

	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"
)

func TestGenerateMatrixNormalizesLegNamesWithoutChangingScenario(t *testing.T) {
	script := TridentE2EScenarioMatrix{}
	data, err := script.GenerateMatrix([]string{"encrypted-partition_vm-host"},
		scenario.HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingCi)
	if err != nil {
		t.Fatal(err)
	}
	var matrix map[string]map[string]string
	if err := json.Unmarshal([]byte(data), &matrix); err != nil {
		t.Fatal(err)
	}
	entry := matrix["encrypted_partition_vm_host"]
	if entry["SCENARIO"] != "encrypted-partition_vm-host" ||
		entry["tridentConfigurationName"] != "encrypted-partition" ||
		entry["installerISOName"] != "trident-installer" {
		t.Fatalf("pipeline facts changed: %v", matrix)
	}
}

func TestGenerateMatrixRejectsCollidingLegNames(t *testing.T) {
	script := TridentE2EScenarioMatrix{}
	_, err := script.GenerateMatrix([]string{"some-config_vm-host", "some_config_vm-host"},
		scenario.HardwareTypeVM, trident.RuntimeTypeHost, testrings.TestRingCi)
	if err == nil || !strings.Contains(err.Error(), "collides") {
		t.Fatalf("expected a leg collision, got %v", err)
	}
}
