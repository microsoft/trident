package trident

import (
	"embed"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v3"
)

//go:generate cp -r ../../../../e2e_tests/trident_configurations configurations
//go:embed configurations/*
var content embed.FS

type tridentConfig struct {
	scenario TridentE2EScenario
	used     bool
}

// Discovers all defined Trident E2E test scenarios.
func DiscoverTridentScenarios(log *logrus.Logger) []TridentE2EScenario {
	entries, err := content.ReadDir("configurations/trident_configurations")
	if err != nil {
		log.Errorf("Failed to read configurations directory: %v", err)
		return nil
	}

	var tridentConfigs = make(map[string]*tridentConfig)

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}

		configPath := "configurations/trident_configurations/" + entry.Name() + "/trident-config.yaml"

		configYaml, err := content.ReadFile(configPath)
		if err != nil {
			log.Fatalf("Failed to read configuration file: %v", err)
		}

		scenario := CreateTridentScenario(entry.Name())

		err = yaml.Unmarshal(configYaml, &scenario.config)
		if err != nil {
			log.Fatalf("Failed to unmarshal configuration file for '%s': %v", scenario.Name(), err)
		}

		tridentConfigs[entry.Name()] = &tridentConfig{scenario: scenario, used: false}
	}

	// Check that all targets exist and that all configs have at least one target
	for stagePath, targets := range STAGE_PATHS_TARGETS {
		for _, target := range targets {
			config, ok := tridentConfigs[target]
			if !ok {
				log.Fatalf("Target '%s' in stage path '%s' references non-existing configuration", target, stagePath)
			}
			config.used = true
			config.scenario.AddStagePath(stagePath.String())
		}
	}

	var scenarios []TridentE2EScenario
	for _, config := range tridentConfigs {
		if !config.used {
			log.Warnf("Configuration '%s' is not used in any stage path", config.scenario.Name())
		}
		scenarios = append(scenarios, config.scenario)
	}

	return scenarios
}

var STAGE_PATHS_TARGETS = map[TridentE2EStagePath][]string{
	NewTridentE2EStagePath(TridentPipelinePr, TridentMachineVm, TridentRuntimeHost): {
		"base",
		"combined",
		"raid-mirrored",
		"raid-resync-small",
		"rerun",
		"simple",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelinePr, TridentMachineVm, TridentRuntimeContainer): {
		"base",
		"combined",
		"raid-mirrored",
		"raid-resync-small",
		"rerun",
		"simple",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelineCi, TridentMachineVm, TridentRuntimeHost): {
		"base",
		"combined",
		"encrypted-partition",
		"encrypted-raid",
		"encrypted-swap",
		"memory-constraint-combined",
		"raid-mirrored",
		"misc",
		"raid-small",
		"raid-resync-small",
		"rerun",
		"simple",
		"verity",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelineCi, TridentMachineVm, TridentRuntimeContainer): {
		"base",
		"combined",
		// "encrypted-partition",  // TODO(9768): Re-enabled once the issue is fixed
		// "encrypted-raid",       // TODO(9768): Re-enabled once the issue is fixed
		// "encrypted-swap",       // TODO(9768): Re-enabled once the issue is fixed
		"raid-mirrored",
		"raid-small",
		"raid-resync-small",
		"rerun",
		"simple",
		"verity",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelinePre, TridentMachineVm, TridentRuntimeHost): {
		"base",
		"combined",
		"encrypted-partition",
		"encrypted-raid",
		"encrypted-swap",
		"memory-constraint-combined",
		"raid-mirrored",
		"misc",
		"raid-small",
		"raid-resync-small",
		"rerun",
		"simple",
		"verity",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelinePre, TridentMachineVm, TridentRuntimeContainer): {
		"base",
		"combined",
		// "encrypted-partition",  // TODO(9768): Re-enabled once the issue is fixed
		// "encrypted-raid",       // TODO(9768): Re-enabled once the issue is fixed
		// "encrypted-swap",       // TODO(9768): Re-enabled once the issue is fixed
		"raid-mirrored",
		"raid-small",
		"raid-resync-small",
		"rerun",
		"simple",
		"verity",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelinePre, TridentMachineBareMetal, TridentRuntimeHost): {
		"base",
		"combined",
		"raid-big",
		"raid-resync-small",
		"encrypted-partition",
		"encrypted-raid",
		"encrypted-swap",
		"memory-constraint-combined",
		"raid-mirrored",
		"rerun",
		"simple",
		"verity",
		"verity-raid",
	},
	NewTridentE2EStagePath(TridentPipelinePre, TridentMachineBareMetal, TridentRuntimeContainer): {
		"base",
		"combined",
		// "encrypted-partition",  # TODO(9768): Re-enabled once the issue is fixed
		// "encrypted-raid",       # TODO(9768): Re-enabled once the issue is fixed
		// "encrypted-swap",       # TODO(9768): Re-enabled once the issue is fixed
		"raid-mirrored",
		"raid-small",
		"raid-resync-small",
		// "rerun",                # TODO(9767): Re-enabled once the issue is fixed
		"simple",
		"verity",
		"verity-raid",
	},
}
