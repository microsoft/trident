package suite

import (
	"storm/pkg/storm"
	"storm/pkg/storm/core"
)

type SuiteContext interface {
	core.Named

	core.LoggerProvider

	// Returns a list of all scenarios
	Scenarios() []storm.Scenario

	// Returns a scenario by name, will exit with an error if the scenario is
	// not found.
	Scenario(name string) storm.Scenario

	// Returns a list of all helpers
	Helpers() []storm.Helper

	// Returns a helper by name, will exit with an error if the helper is
	// not found.
	Helper(name string) storm.Helper
}
