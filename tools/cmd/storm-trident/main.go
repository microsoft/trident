package main

import (
	"tridenttools/storm/e2e"
	"tridenttools/storm/helpers"
	"tridenttools/storm/scripts"
	"tridenttools/storm/servicing"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

func main() {
	storm := storm.CreateSuite("trident")

	// Create a temporary logger for e2e test discovery
	discoveryLogger := logrus.New()
	discoveryLogger.SetLevel(logrus.DebugLevel)

	// Add Trident E2E scenarios
	scenarios, err := e2e.DiscoverTridentScenarios(discoveryLogger)
	if err != nil {
		storm.Logger().Fatalf("Failed to discover Trident E2E scenarios: %v", err)
	}

	for _, scenario := range scenarios {
		storm.AddScenario(&scenario)
	}

	// Add Trident E2E ADO matrix script set
	storm.AddScriptSet(&e2e.TridentE2EMatrixScriptSet{})

	// Add Trident servicing scenario
	storm.AddScenario(&servicing.TridentServicingScenario{})

	// Register Trident helpers
	for _, helper := range helpers.TRIDENT_HELPERS {
		storm.AddHelper(helper)
	}

	// Register Trident scripts
	for _, scriptset := range scripts.TRIDENT_SCRIPTSETS {
		storm.AddScriptSet(scriptset)
	}

	storm.Run()
}
