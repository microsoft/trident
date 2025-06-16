package main

import (
	"storm"
	"tridenttools/storm/helpers"
	"tridenttools/storm/servicing"
)

func main() {
	storm := storm.CreateSuite("trident")

	// Add Trident E2E scenarios (disabled for now)
	// scenarios := trident.DiscoverTridentScenarios(storm.Log)
	// for _, scenario := range scenarios {
	// 	storm.AddScenario(&scenario)
	// }

	// Add Trident servicing scenario
	storm.AddScenario(&servicing.TridentServicingScenario{})

	// Register Trident helpers
	for _, helper := range helpers.TRIDENT_HELPERS {
		storm.AddHelper(helper)
	}

	storm.Run()
}
