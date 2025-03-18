package core

type SetupCleanupContext interface {
	LoggerProvider
	RunnableMetadata
}

type RunnableContext interface {
	LoggerProvider

	TestCaseCreator

	RunnableMetadata
}

type RunnableType int

const (
	RunnableTypeScenario RunnableType = iota
	RunnableTypeHelper
)

func (t RunnableType) String() string {
	switch t {
	case RunnableTypeScenario:
		return "scenario"
	case RunnableTypeHelper:
		return "helper"
	default:
		return "unknown"
	}
}

type RunnableMetadata interface {
	Named

	/// Returns the type of the runnable
	RunnableType() RunnableType
}

type Runnable interface {
	Named

	/// Run the runnable
	Run(RunnableContext) error
}

type SetupCleanupRunnable interface {
	Runnable

	/// Setup before running the runnable
	Setup(SetupCleanupContext) error

	/// Cleanup after running the runnable
	Cleanup(SetupCleanupContext) error
}

type ArgumentedRunnable interface {
	Argumented
	Runnable
}

type ArgumentedSetupCleanupRunnable interface {
	Argumented
	SetupCleanupRunnable
}
