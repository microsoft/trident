package runner

import (
	"fmt"
	"slices"
	"storm/internal/reporter"
	"storm/internal/testmgr"
	"storm/pkg/storm/core"
	"sync"
)

func RegisterAndRunTests(suite core.SuiteContext,
	registrant interface {
		core.Argumented
		core.TestRegistrant
	},
	args []string,
) error {
	// Create a new runnable instance
	registrantInstance := &runnableInstance{
		TestRegistrant: registrant,
		Argumented:     registrant,
	}

	// Parse the extra arguments for the runnable
	err := parseExtraArguments(suite, args, registrantInstance)
	if err != nil {
		return err
	}

	// Create a new test manager for the runnable
	testMgr, err := testmgr.NewStormTestManager(suite, registrantInstance)
	if err != nil {
		return fmt.Errorf("failed to create test manager: %w", err)
	}

	// Actually run the thing
	err = executeTestCases(suite, registrantInstance, testMgr)

	if err != nil {
		switch err.(type) {
		case *setupError:
			// If setup failed we have no test results to report, we can just
			// exist now.
			return err
		case *cleanupError:
			// If cleanup failed we still want to report the test results.
			suite.Logger().Error(err)
		default:
			// Unknown error, log it and continue.
			suite.Logger().WithError(err).Error("Unknown error occurred!")
		}
	}

	rep := reporter.NewTestReporter(testMgr)

	rep.PrintReport()

	return rep.ExitError()
}

func executeTestCases(suite core.SuiteContext,
	runnable interface {
		core.TestRegistrantMetadata
		core.TestRegistrant
	},
	testManager *testmgr.StormTestManager,
) error {

	ctx := &runnableContext{
		LoggerProvider:         suite,
		TestRegistrantMetadata: runnable,
	}

	// If the runnable implements the SetupCleanup interface, we call
	// the setup method before running the tests.
	if r, ok := runnable.(core.SetupCleanup); ok {
		err := runCatchPanic(func() error { return r.Setup(ctx) })
		if err != nil {
			return newSetupError(runnable, err)
		}
	}

	cleanupFuncs := make([]func(), 0)

	bail := false

	for _, testCase := range testManager.TestCases() {
		if !bail {
			suite.Logger().Infof("%s (started)", testCase.Name())
			// Run the test case.
			executeTestCase(testCase)

			// Grab and store the cleanup functions for this test case.
			cleanupFuncs = append(cleanupFuncs, testCase.SuiteCleanupList()...)

			// Check if the test case caused a bail condition.
			bail = testCase.IsBailCondition()
			suite.Logger().Infof("%s %s", testCase.Name(), testCase.Status().ColorString())
		} else {
			testCase.MarkNotRun("dependency failure")
		}

	}

	// If we have any cleanup functions, run them in reverse order.
	slices.Reverse(cleanupFuncs)
	for _, f := range cleanupFuncs {
		runCatchPanic(func() error {
			f()
			return nil
		})
	}

	// If the runnable implements the SetupCleanup interface, we call
	// the Cleanup method after running the tests.
	if r, ok := runnable.(core.SetupCleanup); ok {
		err := runCatchPanic(func() error { return r.Cleanup(ctx) })
		if err != nil {
			return newCleanupError(runnable, err)
		}
	}

	return nil
}

func executeTestCase(testCase *testmgr.TestCase) {
	var err error
	var wg sync.WaitGroup

	// Run the runnable in a separate goroutine to so that runtime.Goexit() can
	// be called to stop the test execution.
	wg.Add(1)
	go func() {
		defer wg.Done()
		err = runCatchPanic(func() error {
			return testCase.Execute()
		})
	}()

	// Wait for the goroutine to finish and close the test with whatever
	// error we receive, if any.
	wg.Wait()

	if err != nil {
		testCase.MarkError(err)
	} else if testCase.Status().IsRunning() {
		testCase.Pass()
	}
}

type PanicError struct {
	any
}

func (pe PanicError) Error() string {
	return fmt.Sprintf("panic occurred: %v", pe.any)
}

func runCatchPanic(f func() error) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = PanicError{r}
		}
	}()

	return f()
}
