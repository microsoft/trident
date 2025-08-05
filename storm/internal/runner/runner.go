package runner

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"runtime/debug"
	"slices"
	"storm/internal/reporter"
	"storm/internal/stormerror"
	"storm/internal/testmgr"
	"storm/pkg/storm/core"
	"sync"

	"github.com/sirupsen/logrus"
)

func RegisterAndRunTests(suite core.SuiteContext,
	registrant interface {
		core.Argumented
		core.TestRegistrant
	},
	args []string,
	watch bool,
	logDir *string,
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
	err = executeTestCases(suite, registrantInstance, testMgr, watch)

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

	if logDir != nil {
		suite.Logger().Infof("Saving logs to '%s'", *logDir)
		err := os.MkdirAll(*logDir, 0755)
		if err != nil {
			return fmt.Errorf("failed to create log directory '%s': %w", *logDir, err)
		}
		rep.SaveLogs(*logDir)
	}

	return rep.ExitError()
}

func executeTestCases(suite core.SuiteContext,
	runnable *runnableInstance,
	testManager *testmgr.StormTestManager,
	watch bool,
) error {

	ctx := &runnableContext{
		LoggerProvider:         suite,
		TestRegistrantMetadata: runnable,
	}

	// If the runnable implements the SetupCleanup interface, we call
	// the setup method before running the tests.
	if r, ok := runnable.TestRegistrant.(core.SetupCleanup); ok {
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
			captured, err := captureOutput(func() {
				executeTestCase(testCase)
			}, func(w io.Writer, s string) {
				if suite.AzureDevops() || watch {
					fmt.Fprintf(w, "  ├ %s\n", s)
				}
			})

			// Store the captured output in the test case.
			testCase.SetCollectedOutput(captured)

			// If we failed to collect the output, return an error. This means
			// that we didn't even run.
			if err != nil {
				return fmt.Errorf("failed to capture output for '%s': %w", testCase.Name(), err)
			}

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
	if r, ok := runnable.TestRegistrant.(core.SetupCleanup); ok {
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

func runCatchPanic(f func() error) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = stormerror.NewPanicError(r, debug.Stack())
		}
	}()

	return f()
}

func captureOutput(f func(), forward func(io.Writer, string)) ([]string, error) {
	oldStdout := os.Stdout
	oldStderr := os.Stderr

	rOut, wOut, err := os.Pipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stdout capture pipe: %w", err)
	}

	rErr, wErr, err := os.Pipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stderr capture pipe: %w", err)
	}

	os.Stdout = wOut
	os.Stderr = wErr

	logrusOutput := logrus.StandardLogger().Out
	logrusFormatter := logrus.StandardLogger().Formatter
	logrusLevel := logrus.StandardLogger().Level

	// Logrust's standard logger is created on startup and stores a reference to
	// the real stderr then, so our clever redirection does not work. To enable it, we
	// need to set the output of the logger to our pipe as well.
	logrus.SetOutput(os.Stderr)

	// Trick logrus into treating our pipe as the real stderr and force it to TRACE level.
	logrus.SetFormatter(&logrus.TextFormatter{
		ForceColors: true,
	})
	logrus.SetLevel(logrus.TraceLevel)

	defer func() {
		os.Stdout = oldStdout
		os.Stderr = oldStderr

		// Restore the original logrus configuration
		logrus.SetOutput(logrusOutput)
		logrus.SetFormatter(logrusFormatter)
		logrus.SetLevel(logrusLevel)
	}()

	var combinedOutput []string
	var outMutex sync.Mutex
	var wg sync.WaitGroup

	var streamReader = func(r io.Reader, w io.Writer) {
		defer wg.Done()
		scanner := bufio.NewScanner(r)
		for scanner.Scan() {
			line := scanner.Text()
			outMutex.Lock()
			combinedOutput = append(combinedOutput, line)
			outMutex.Unlock()
			forward(w, line)
		}
	}

	wg.Add(2)

	go streamReader(rOut, oldStdout)
	go streamReader(rErr, oldStderr)

	f()

	wOut.Close()
	wErr.Close()

	wg.Wait()

	return combinedOutput, nil
}
