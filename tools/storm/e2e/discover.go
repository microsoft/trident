package e2e

import (
	"embed"
	"fmt"
	"tridenttools/storm/e2e/scenario"

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

		var config map[string]any
		err = yaml.Unmarshal(configYaml, &config)
		if err != nil {
			log.Fatalf("Failed to unmarshal configuration file for '%s': %v", name, err)
		}

		scenarios := produceScenariosFromConfig(name, conf, config)
		tridentE2EScenarios = append(tridentE2EScenarios, scenarios...)
	}

	return tridentE2EScenarios, nil
}

func getConfigPath(scenarioName string) string {
	return "configurations/trident_configurations/" + scenarioName + "/trident-config.yaml"
}

func produceScenariosFromConfig(name string, conf scenarioConfig, config map[string]interface{}) []scenario.TridentE2EScenario {
	var scenarios []scenario.TridentE2EScenario

	bmScenario := produceScenario(name, config, scenario.HardwareTypeBM, scenario.RuntimeTypeHost, conf.Bm.Host)
	if bmScenario != nil {
		scenarios = append(scenarios, *bmScenario)
	}

	bmContainerScenario := produceScenario(name, config, scenario.HardwareTypeBM, scenario.RuntimeTypeContainer, conf.Bm.Container)
	if bmContainerScenario != nil {
		scenarios = append(scenarios, *bmContainerScenario)
	}

	vmScenario := produceScenario(name, config, scenario.HardwareTypeVM, scenario.RuntimeTypeHost, conf.Vm.Host)
	if vmScenario != nil {
		scenarios = append(scenarios, *vmScenario)
	}

	vmContainerScenario := produceScenario(name, config, scenario.HardwareTypeVM, scenario.RuntimeTypeContainer, conf.Vm.Container)
	if vmContainerScenario != nil {
		scenarios = append(scenarios, *vmContainerScenario)
	}

	return scenarios
}

func produceScenario(
	name string,
	config map[string]interface{},
	hardware scenario.HardwareType,
	runtime scenario.RuntimeType,
	lowest_ring testRing,
) *scenario.TridentE2EScenario {
	rings := lowest_ring.GetTargetList()

	if len(rings) == 0 {
		return nil
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
	)
}

type configs map[string]scenarioConfig

type scenarioConfig struct {
	Bm runtimeConfig `yaml:"bm"`
	Vm runtimeConfig `yaml:"vm"`
}

type runtimeConfig struct {
	Host      testRing `yaml:"host"`
	Container testRing `yaml:"container"`
}

type testRing string

const (
	TestRingPrE2e          testRing = "pr-e2e"
	TestRingCi             testRing = "ci"
	TestRingPre            testRing = "pre"
	TestRingFullValidation testRing = "full-validation"
)

var pipelineRingsOrder = []testRing{
	TestRingPrE2e,
	TestRingCi,
	TestRingPre,
	TestRingFullValidation,
}

func (tr testRing) GetTargetList() []testRing {
	var targets []testRing
	found := false
	for _, ring := range pipelineRingsOrder {
		if ring == tr {
			found = true
		}
		if found {
			targets = append(targets, ring)
		}
	}
	return targets
}
