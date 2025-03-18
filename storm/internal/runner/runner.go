package runner

import (
	"storm/internal/reporter"
	"storm/internal/testmgr"
	"storm/pkg/storm/core"
)

func RunRunnable(suite core.SuiteContext,
	runnable core.ArgumentedRunnable,
	args []string,
) error {
	// Create a new runnable instance
	runnableInstance := &runnableInstance{
		ArgumentedRunnable: runnable,
	}

	// Parse the extra arguments for the runnable
	err := parseExtraArguments(suite, args, runnableInstance)
	if err != nil {
		return err
	}

	// Create a new test manager for the runnable
	testMgr := testmgr.NewStormTestManager(suite, runnableInstance)

	ctx := &runnableContext{
		runnableMeta: runnableInstance,
		logger:       suite.Logger(),
		testCreator:  testMgr,
	}

	// Actually run the thing
	err = executeRunnableInner(suite, runnableInstance, testMgr, ctx)

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
