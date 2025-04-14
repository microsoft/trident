package core

type SetupCleanupContext interface {
	LoggerProvider
	TestRegistrantMetadata
}

type SetupCleanup interface {
	/// Setup before running the runnable
	Setup(SetupCleanupContext) error

	/// Cleanup after running the runnable
	Cleanup(SetupCleanupContext) error
}
