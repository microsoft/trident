package testmgr

import (
	"storm/pkg/storm/core"
	"time"
)

type StormTestManager struct {
	suite     core.SuiteContext
	runnable  core.RunnableMetadata
	startTime time.Time
	testCases []*TestCase
}

func NewStormTestManager(suite core.SuiteContext, runnable core.RunnableMetadata) *StormTestManager {
	return &StormTestManager{
		suite:     suite,
		startTime: time.Now(),
		testCases: make([]*TestCase, 0),
		runnable:  runnable,
	}
}

func (r *StormTestManager) NewTestCase(name string) core.TestCase {
	if len(r.testCases) > 0 {
		lastTestCase := r.testCases[len(r.testCases)-1]
		// Implicitly pass the last test case if it is still running.
		if lastTestCase.isRunning() {
			lastTestCase.Pass()
		}
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
			lastTestCase.markError(err)
		} else {
			lastTestCase.markPass()
		}
	}
}

func (r *StormTestManager) TestCases() []*TestCase {
	return r.testCases
}

func (r *StormTestManager) Suite() core.SuiteContext {
	return r.suite
}

func (r *StormTestManager) Runnable() core.RunnableMetadata {
	return r.runnable
}
