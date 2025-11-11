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

// Discovers all defined Trident E2E test scenarios.
func DiscoverTridentScenarios(log *logrus.Logger) ([]scenario.TridentE2EScenario, error) {
	rawConfigs, err := content.ReadFile("configurations/configurations.yaml")
	if err != nil {
		return nil, fmt.Errorf("failed to read configurations file: %w", err)
	}

	var cfg configs
	err = yaml.Unmarshal(rawConfigs, &cfg)
	if err != nil {
		return nil, fmt.Errorf("failed to parse e2e configurations file: %w", err)
	}

	var tridentE2EScenarios []scenario.TridentE2EScenario = make([]scenario.TridentE2EScenario, 0, len(cfg))

	for name, conf := range cfg {
		configPath := getConfigPath(name)

		configYaml, err := content.ReadFile(configPath)
		if err != nil {
			log.Fatalf("Failed to read configuration file: %v", err)
		}

		var hostConfig map[string]any
		err = yaml.Unmarshal(configYaml, &hostConfig)
		if err != nil {
			log.Fatalf("Failed to unmarshal configuration file for '%s': %v", name, err)
		}

		scenarios, err := produceScenariosFromConfig(name, conf, hostConfig)
		if err != nil {
			log.Fatalf("Failed to produce scenarios for '%s': %v", name, err)
		}

		tridentE2EScenarios = append(tridentE2EScenarios, scenarios...)
	}

	return tridentE2EScenarios, nil
}

func getConfigPath(scenarioName string) string {
	return "configurations/trident_configurations/" + scenarioName + "/trident-config.yaml"
}

func produceScenariosFromConfig(name string, conf scenarioConfig, hostConfig map[string]interface{}) ([]scenario.TridentE2EScenario, error) {
	var scenarios []scenario.TridentE2EScenario

	groups := []struct {
		hardware scenario.HardwareType
		runtime  scenario.RuntimeType
		ring     testrings.TestRing
	}{
		{scenario.HardwareTypeBM, scenario.RuntimeTypeHost, conf.Bm.Host},
		{scenario.HardwareTypeBM, scenario.RuntimeTypeContainer, conf.Bm.Container},
		{scenario.HardwareTypeVM, scenario.RuntimeTypeHost, conf.Vm.Host},
		{scenario.HardwareTypeVM, scenario.RuntimeTypeContainer, conf.Vm.Container},
	}

	for _, group := range groups {
		scenario, err := produceScenario(name, hostConfig, group.hardware, group.runtime, group.ring)
		if err != nil {
			return nil, err
		}
		if scenario != nil {
			scenarios = append(scenarios, *scenario)
		}
	}

	return scenarios, nil
}

func produceScenario(
	name string,
	config map[string]interface{},
	hardware scenario.HardwareType,
	runtime scenario.RuntimeType,
	lowest_ring testrings.TestRing,
) (*scenario.TridentE2EScenario, error) {
	rings, err := lowest_ring.GetTargetList()
	if err != nil {
		return nil, err
	}

	if len(rings) == 0 {
		return nil, nil
	}

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

type configs map[string]scenarioConfig

type scenarioConfig struct {
	Bm runtimeConfig `yaml:"bm"`
	Vm runtimeConfig `yaml:"vm"`
}

type runtimeConfig struct {
	Host      testrings.TestRing `yaml:"host"`
	Container testrings.TestRing `yaml:"container"`
}
