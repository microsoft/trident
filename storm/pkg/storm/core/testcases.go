package core

import "time"

type TestCase interface {
	Named

	// Returns information about the registrant that created this test case.
	Registrant() TestRegistrantMetadata

	// Fail the test case. Implementations will stop execution by calling
	// runtime.Goexit(), which then runs all deferred calls in the current
	// goroutine.
	Fail(reason string)

	// Fail the test case with an error. Implementations will stop execution by
	// calling runtime.Goexit(), which then runs all deferred calls in the
	// current goroutine.
	FailFromError(err error)

	// Error the test case. Implementations will stop execution by calling
	// runtime.Goexit(), which then runs all deferred calls in the current
	// goroutine.
	Error(err error)

	// Skip the test case. Implementations will stop execution by calling
	// runtime.Goexit(), which then runs all deferred calls in the current
	// goroutine.
	Skip(reason string)

	// Get the test case run time
	RunTime() time.Duration

	// Registers a cleanup function to be called after all subsequent test cases
	// in the suite have finished, regardless of their status. Cleanup functions
	// are called in reverse order of registration.
	SuiteCleanup(f func())
}
