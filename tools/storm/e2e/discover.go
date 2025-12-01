package e2e

import (
	"embed"
	"fmt"
	"tridenttools/storm/e2e/scenario"
	"tridenttools/storm/e2e/testrings"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v3"
)

const (
	SCENARIO_TAG_E2E = "e2e"
)

//go:generate cp -r ../../../tests/e2e_tests/trident_configurations configurations
//go:generate python3 invert.py
//go:embed configurations/*
var content embed.FS

// Discovers all defined Trident E2E test scenarios and returns them as a list.
func DiscoverTridentScenarios(log *logrus.Logger) ([]scenario.TridentE2EScenario, error) {
	// Read the ring configurations file and unmarshal it.
	rawConfigs, err := content.ReadFile("configurations/configurations.yaml")
	if err != nil {
		return nil, fmt.Errorf("failed to read configurations file: %w", err)
	}

	var cfg configs
	err = yaml.Unmarshal(rawConfigs, &cfg)
	if err != nil {
		return nil, fmt.Errorf("failed to parse e2e configurations file: %w", err)
	}

	// Accumulate all scenarios here.
	var tridentE2EScenarios []scenario.TridentE2EScenario = make([]scenario.TridentE2EScenario, 0, len(cfg))

	// Iterate over all top-level host configurations and produce all relevant scenarios.
	for name, conf := range cfg {
		// Get the path to the host configuration file for this scenario, read it and unmarshal it to a map.
		configPath := getConfigPath(name)

		configYaml, err := content.ReadFile(configPath)
		if err != nil {
			log.Fatalf("Failed to read configuration file: %v", err)
		}

		var hostConfig map[string]any
		err = yaml.Unmarshal(configYaml, &hostConfig)
		if err != nil {
			return nil, fmt.Errorf("failed to unmarshal configuration file for '%s': %v", name, err)
		}

		// Produce scenarios from this configuration.
		scenarios, err := produceScenariosFromConfig(name, conf, hostConfig)
		if err != nil {
			return nil, fmt.Errorf("failed to produce scenarios for '%s': %v", name, err)
		}

		tridentE2EScenarios = append(tridentE2EScenarios, scenarios...)
	}

	return tridentE2EScenarios, nil
}

// Returns the path to the configuration file for the given scenario name.
func getConfigPath(scenarioName string) string {
	return "configurations/trident_configurations/" + scenarioName + "/trident-config.yaml"
}

// Produces all scenarios from a given configuration.
func produceScenariosFromConfig(name string, conf scenarioConfig, hostConfig map[string]interface{}) ([]scenario.TridentE2EScenario, error) {
	var scenarios []scenario.TridentE2EScenario

	// Iterate over all hardware types
	for _, hw := range scenario.HardwareTypes() {
		// For the current hardware type, get the corresponding runtime configuration from the scenario config
		var currentRtConfig runtimeConfig
		switch hw {
		case scenario.HardwareTypeBM:
			currentRtConfig = conf.Bm
		case scenario.HardwareTypeVM:
			currentRtConfig = conf.Vm
		}

		// Iterate over all runtime types
		for _, rt := range scenario.RuntimeTypes() {
			// For the current runtime type, get the corresponding test ring from the runtime configuration
			var ring testrings.TestRing
			switch rt {
			case scenario.RuntimeTypeHost:
				ring = currentRtConfig.Host
			case scenario.RuntimeTypeContainer:
				ring = currentRtConfig.Container
			}

			// Produce the scenario for this hardware/runtime/ring combination
			scenario, err := produceScenario(name, hostConfig, hw, rt, ring)
			if err != nil {
				return nil, err
			}

			// Append the scenario if it was produced (i.e., not nil)
			if scenario != nil {
				scenarios = append(scenarios, *scenario)
			}
		}
	}

	return scenarios, nil
}

// Conditionally produces a single scenario for the given parameters. Assuming
// that the scenario configuration declares that the provided host configuration
// on the provided hardware/runtime should be run at the provided lowest
// pipeline ring, a scenario is produced and returned. If the lowest pipeline
// ring is 'none' or 'empty', nil is returned.
//
// A nil value indicates that this HC/HW/RT combination should not be run at all
// in any ring.
func produceScenario(
	name string,
	config map[string]interface{},
	hardware scenario.HardwareType,
	runtime scenario.RuntimeType,
	lowest_ring testrings.TestRing,
) (*scenario.TridentE2EScenario, error) {
	// TEMPORARILY ONLY ENABLE VM/HOST SCENARIOS
	if hardware != scenario.HardwareTypeVM || runtime != scenario.RuntimeTypeHost {
		return nil, nil
	}

	// Get the list of all target rings for this scenario. This is the list of
	// rings from the lowest declared ring up to the highest existing ring.
	// For example, if the lowest ring is 'ci', the returned list will be
	// ['ci', 'pre', 'full-validation'].
	//
	// If the lowest ring is 'none' or empty string, an empty list is returned.
	rings, err := lowest_ring.GetTargetList()
	if err != nil {
		return nil, err
	}

	// If the list of rings is empty, this configuration should not be run
	// at all, so return nil.
	if len(rings) == 0 {
		return nil, nil
	}

	// Build the scenario object, first building the list of tags. Then call the
	// constructor.
	tags := []string{SCENARIO_TAG_E2E, hardware.ToString(), runtime.ToString()}
	for _, ring := range rings {
		tags = append(tags, string(ring))
	}

	return scenario.NewTridentE2EScenario(
		fmt.Sprintf("%s_%s-%s", name, hardware, runtime),
		tags,
		config,
		hardware,
		runtime,
		rings,
	), nil
}

// Top level types for unmarshaling the configurations.yaml file.
type configs map[string]scenarioConfig

type scenarioConfig struct {
	Bm runtimeConfig `yaml:"bm"`
	Vm runtimeConfig `yaml:"vm"`
}

type runtimeConfig struct {
	Host      testrings.TestRing `yaml:"host"`
	Container testrings.TestRing `yaml:"container"`
}
