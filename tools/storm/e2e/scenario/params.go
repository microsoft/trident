package scenario

import (
	"fmt"
	"strings"

	"tridenttools/storm/utils/trident"
)

type HardwareType string

const (
	HardwareTypeBM HardwareType = "bm"
	HardwareTypeVM HardwareType = "vm"
)

func (ht HardwareType) ToString() string {
	return string(ht)
}

func (ht HardwareType) IsVM() bool {
	return ht == HardwareTypeVM
}

func (ht HardwareType) IsBM() bool {
	return ht == HardwareTypeBM
}

func HardwareTypes() []HardwareType {
	return []HardwareType{HardwareTypeBM, HardwareTypeVM}
}

// DeploymentEnvironment is the name this hardware type is recorded under in the
// Kusto telemetry tables and in the ACR image tags. It differs from ToString()
// because those values predate storm and must keep matching the legacy suite's,
// or storm-produced rows stop grouping with the existing ones.
func (ht HardwareType) DeploymentEnvironment() string {
	if ht.IsBM() {
		return "bareMetal"
	}
	return "virtualMachine"
}

// ScenarioName composes the scenario name for a configuration, matching what
// discover.go registers.
func ScenarioName(config string, ht HardwareType, rt trident.RuntimeType) string {
	return fmt.Sprintf("%s_%s-%s", config, ht, rt)
}

// ParseScenarioName splits a scenario name back into the configuration name and
// the hardware/runtime it targets. It is the inverse of ScenarioName.
func ParseScenarioName(name string) (config string, ht HardwareType, rt trident.RuntimeType, err error) {
	underscore := strings.LastIndex(name, "_")
	if underscore < 1 || underscore == len(name)-1 {
		return "", "", "", fmt.Errorf("scenario name %q is not <config>_<hardware>-<runtime>", name)
	}

	config, suffix := name[:underscore], name[underscore+1:]
	hw, runtime, found := strings.Cut(suffix, "-")
	if !found {
		return "", "", "", fmt.Errorf("scenario name %q is not <config>_<hardware>-<runtime>", name)
	}

	ht = HardwareType(hw)
	if !ht.IsVM() && !ht.IsBM() {
		return "", "", "", fmt.Errorf("scenario name %q has unknown hardware type %q", name, hw)
	}

	rt = trident.RuntimeType(runtime)
	if rt != trident.RuntimeTypeHost && rt != trident.RuntimeTypeContainer {
		return "", "", "", fmt.Errorf("scenario name %q has unknown runtime type %q", name, runtime)
	}

	return config, ht, rt, nil
}
