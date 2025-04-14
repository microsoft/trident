package core

type Scenario interface {
	Argumented
	SetupCleanup
	TestRegistrant

	// Tags associated with the scenario, the implementation should ensure that
	// the tags are unique.
	Tags() []string

	// List of files that are expected to be always present and exist for the
	// scenario to be able to run.
	RequiredFiles() []string

	// List of all stages that include this scenario.
	StagePaths() []string
}

// BaseScenario is a partial implementation of the Scenario interface. It is
// meant to be used for composition when not all methods of the Scenario
// interface are needed. It does NOT provide a default implementation for the
// Name() and RegisterTestCases() methods.
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

func (s BaseScenario) Setup(SetupCleanupContext) error {
	return nil
}

func (s BaseScenario) Cleanup(SetupCleanupContext) error {
	return nil
}
