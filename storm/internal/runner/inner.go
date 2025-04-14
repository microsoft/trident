package runner

import (
	"fmt"
	"storm/internal/testmgr"
	"storm/pkg/storm/core"
)

func executeRunnableInner(suite core.SuiteContext,
	runnable interface {
		core.RunnableMetadata
		core.ArgumentedRunnable
	},
	testManager *testmgr.StormTestManager,
	runnableContext *runnableContext,
) error {
	// If the runnable implements the SetupCleanupRunnable interface, we call
	// the setup method before running the test.
	if r, ok := runnable.(core.SetupCleanupRunnable); ok {
		err := r.Setup(runnableContext)
		if err != nil {
			return newSetupError(runnable, err)
		}
	}

	// Create an error channel to handle the test execution.
	errChan := make(chan error)

	// Run the runnable in a separate goroutine to so that runtime.Goexit() can
	// be called to stop the test execution.
	go func() {
		// Defer a panic recovery function to ensure that we produce an error
		// if the test panics.
		defer func() {
			if r := recover(); r != nil {
				// Send the panic value to the errChan channel.
				errChan <- fmt.Errorf("panic occurred in runnable: %v", r)
			} else {
				// Send nil to the errChan channel if no panic occurred.
				errChan <- nil
			}
		}()

		// Run the runnable and send the result to the errChan channel.
		errChan <- runnable.Run(runnableContext)
	}()

	// Wait for the goroutine to finish and close the test manager with whatever
	// error we receive, if any.
	err := testManager.Close(<-errChan)

	// If the runnable implements the SetupCleanupRunnable interface, we call
	// the Cleanup method after running the test.
	if r, ok := runnable.(core.SetupCleanupRunnable); ok {
		err := r.Cleanup(runnableContext)
		if err != nil {
			return newCleanupError(runnable, err)
		}
	}

	return err
}
