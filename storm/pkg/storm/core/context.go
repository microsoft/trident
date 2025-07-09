package core

import "context"

type SuiteContext interface {
	Named

	LoggerProvider

	// Returns a list of all scenarios
	Scenarios() []Scenario

	// Returns a scenario by name, will exit with an error if the scenario is
	// not found.
	Scenario(name string) Scenario

	// Returns a list of all helpers
	Helpers() []Helper

	// Returns a helper by name, will exit with an error if the helper is
	// not found.
	Helper(name string) Helper

	// Returns whether the suite has Azure DevOps integration enabled
	AzureDevops() bool

	// Returns a context for the suite.
	Context() context.Context
}
