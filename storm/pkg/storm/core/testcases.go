package core

import "time"

type TestCaseCreator interface {
	NewTestCase(name string) TestCase
}

type TestCase interface {
	Named

	LoggerProvider

	// Explicitly mark the test case as passed. Test cases are also implicitly marked as passed if
	// the suite finishes without any errors, or the next test case is started.
	Pass()

	// Fail the test case. Implementations will stop execution by calling
	// runtime.Goexit().
	Fail(reason string)

	// Fail the test case with an error. Implementations will stop execution by
	// calling runtime.Goexit().
	FailFromError(err error)

	// Error the test case. Implementations will stop execution by calling
	// runtime.Goexit().
	Error(err error)

	// Skip the test case. Implementations will stop execution by calling
	// runtime.Goexit().
	Skip(reason string)

	// Skip this test case and continue the test case. Implementations will not
	// stop execution. This is useful for test cases that are not mandatory to
	// pass.
	SkipAndContinue(reason string)

	// Get the test case run time
	RunTime() time.Duration
}
