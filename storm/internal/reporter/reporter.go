package reporter

import (
	"fmt"
	"storm/pkg/storm/core"
	"strings"
	"time"

	"golang.org/x/term"
)

type StormTestManager struct {
	suite     core.SuiteContext
	startTime time.Time
	testCases []*TestCase
}

type testSummary struct {
	total   int
	passed  int
	failed  int
	skipped int
}

func NewStormTestManager(suite core.SuiteContext) StormTestManager {
	return StormTestManager{
		suite:     suite,
		startTime: time.Now(),
		testCases: make([]*TestCase, 0),
	}
}

func (r *StormTestManager) NewTestCase(name string) core.TestCase {
	if len(r.testCases) > 0 {
		lastTestCase := r.testCases[len(r.testCases)-1]
		// Implicitly pass the last test case
		lastTestCase.Pass()
	}

	textCase := newTestCase(name, uint(len(r.testCases)), r)

	r.testCases = append(r.testCases, textCase)
	return textCase
}

// Closes the reporter and attaches any error to the last test case if it is
// still running.
//
// When error is nil, it is ignored.
func (r *StormTestManager) Close(err error) {
	r.suite.Logger().Debug("Closing reporter")
	if len(r.testCases) == 0 {
		return
	}

	// Get the last test case and check if it is still running. If so, it means
	// the test case was not explicitly marked as passed or failed.
	// Depending on the error, this could mean:
	// - err is nil: the test is an implicit pass
	// - err is not nil: the test did not finish correctly because of an error
	lastTestCase := r.testCases[len(r.testCases)-1]
	if lastTestCase.isRunning() {
		if err != nil {
			lastTestCase.Error(err)
		} else {
			lastTestCase.Pass()
		}
	}
}

func (r *StormTestManager) summary() testSummary {
	var summary testSummary

	for _, testCase := range r.testCases {
		summary.total++
		switch testCase.status {
		case TestCaseStatusPassed:
			summary.passed++
		case TestCaseStatusFailed:
			summary.failed++
		case TestCaseStatusSkipped:
			summary.skipped++
		default:
			panic("Invalid test case status")
		}
	}

	return summary
}

func (r *StormTestManager) PrintReport() error {
	summary := r.summary()

	if summary.total == 0 {
		return fmt.Errorf("no test cases were run")
	}

	width, _, err := term.GetSize(0)
	if err != nil {
		width = 80
	}

	for _, testCase := range r.testCases {
		if testCase.passed() {
			continue
		}

		fmt.Println(strings.Repeat("-", width))
		fmt.Printf(
			"Test case: '%s' status: %s; collected logs:\n",
			testCase.name,
			testCase.status.String(),
		)
		for _, log := range testCase.logLines() {
			fmt.Println("    ", log)
		}
	}

	var status = "ok"
	if summary.failed > 0 {
		status = "failed"
	}

	fmt.Println(strings.Repeat("-", width))
	fmt.Printf(
		"TEST RESULT: %s. %d total; %d failed; %d skipped; %d passed\n",
		status,
		summary.total,
		summary.failed,
		summary.skipped,
		summary.passed,
	)

	return nil
}

func (r *StormTestManager) ExitError() error {
	summary := r.summary()

	if summary.failed > 0 {
		return fmt.Errorf("test suite finished with %d failed test cases", summary.failed)
	}

	return nil
}
