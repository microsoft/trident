package run

import (
	"fmt"
	"storm/internal/reporter"
	"storm/pkg/storm/core"
	"sync"
)

func runTestRunnable(suite core.SuiteContext,
	argumented core.Argumented,
	args []string, name string,
	kind runnableKind,
	run func(tcc core.TestCaseCreator) error,
) error {
	parseExtraArguments(suite, argumented, args, name, kind)

	rep := reporter.NewStormTestManager(suite)

	var wg sync.WaitGroup
	var errChan chan error = make(chan error)
	wg.Add(1)
	go func() {
		defer wg.Done()
		err := run(&rep)

		// Only send the error if it is not nil.
		if err != nil {
			errChan <- err
		}
	}()

	wg.Wait()

	select {
	case err := <-errChan:
		rep.Close(err)
	default:
		// No error was produced, the goroutine was most likely killed by a
		// failed test case.
		rep.Close(nil)
	}

	err := rep.PrintReport()
	if err != nil {
		return fmt.Errorf("failed to print report: %v", err)
	}

	return rep.ExitError()
}
