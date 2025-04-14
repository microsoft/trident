package main

import (
	"storm/pkg/storm"
	trident "storm/suites/trident/e2e"
	"storm/suites/trident/helpers"
)

func main() {
	storm := storm.CreateSuite("trident")

	// Add Trident E2E scenarios
	scenarios := trident.DiscoverTridentScenarios(storm.Log)
	for _, scenario := range scenarios {
		storm.AddScenario(&scenario)
	}

	// Register Trident helpers
	for _, helper := range helpers.TRIDENT_HELPERS {
		storm.AddHelper(helper)
	}

	storm.Run()
}
