package core

type Scenario interface {
	Named
	Argumented

	// Tags associated with the scenario, the implementation should ensure that
	// the tags are unique.
	Tags() []string

	// List of files that are expected to be always present and exist for the
	// scenario to be able to run.
	RequiredFiles() []string

	// List of all stages that include this scenario.
	StagePaths() []string

	// Returns a pointer to an instance of a kong-annotated struct to parse
	// additional command line arguments into.
	Args() any

	// Setup - This is called before the scenario is run.
	Setup(ScenarioContext) error

	// Run the scenario
	Run(ScenarioContext) error

	// Cleanup - This is called after the scenario is run.
	Cleanup(ScenarioContext) error
}

// BaseScenario is a partial implementation of the Scenario interface. It is
// meant to be used for composition when not all methods of the Scenario
// interface are needed. It does NOT provide a default implementation for the
// Name() and Run() methods.
type BaseScenario struct{}

func (s BaseScenario) Tags() []string {
	return nil
}

func (s BaseScenario) RequiredFiles() []string {
	return nil
}

func (s BaseScenario) StagePaths() []string {
	return nil
}

func (s BaseScenario) Args() any {
	return nil
}

func (s BaseScenario) Setup(ScenarioContext) error {
	return nil
}

func (s BaseScenario) Cleanup(ScenarioContext) error {
	return nil
}
