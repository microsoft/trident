package testmgr

import (
	"context"
	"fmt"
	"runtime"
	"storm/pkg/storm/core"
	"sync"
	"time"
)

type TestCase struct {
	ctx             context.Context
	cancel          context.CancelFunc
	registrant      core.TestRegistrantMetadata
	name            string
	startTime       time.Time
	endTime         time.Time
	status          TestCaseStatus
	reason          string
	err             error
	collectedOutput []string
	f               core.TestCaseFunction
	suiteCleanup    []func()
	waitGroup       sync.WaitGroup
}

func newTestCase(name string, f core.TestCaseFunction, ctx context.Context) *TestCase {
	tc_ctx, cancel := context.WithCancel(ctx)
	tc := &TestCase{
		name:   name,
		f:      f,
		status: TestCaseStatusPending,
		ctx:    tc_ctx,
		cancel: cancel,
	}

	return tc
}

// Executes the test case function. The returned error is guaranteed to be the
// return of the test case function.
func (t *TestCase) Execute() error {
	if t.f == nil {
		panic(fmt.Sprintf("Test case '%s' has no runnable function", t.name))
	}

	t.startTime = time.Now()
	t.status = TestCaseStatusRunning

	return t.f(t)
}

func (t *TestCase) close(status TestCaseStatus, reason string, err error) {
	// Cancel the context to stop any goroutines that might be running
	t.cancel()

	// If the test case is still running, we need to wait for it to finish.
	t.waitGroup.Wait()

	if !status.IsFinal() {
		panic("Attempted to close test with non-final status: " + status.String())
	}

	// If the current status is final, we cannot change it.
	if t.status.IsFinal() {
		panic(fmt.Sprintf("Test case '%s' is already closed with status '%s'", t.name, t.status.String()))
	}

	// If the current status is pending, we can only close it with a skipped status.
	if t.status == TestCaseStatusPending {
		if status != TestCaseStatusNotRun {
			panic(fmt.Sprintf("Pending test case can only be closed with a '%s' status", TestCaseStatusNotRun.String()))
		}
	}

	// Update the status and end time
	t.status = status
	t.endTime = time.Now()

	// Set the reason for the test case closure
	if err != nil {
		// Store the error and its string representation as the reason
		t.err = err
		t.reason = err.Error()
	} else {
		t.reason = reason
	}
}

// Returns whether this test caused a bail condition, which means that the test
// suite should stop. This is true if the test failed or errored out in a way
// that does not allow for recovery.
func (t *TestCase) IsBailCondition() bool {
	return t.status.IsBad()
}

// Returns the collected output of the test case.
func (t *TestCase) CollectedOutput() []string {
	return t.collectedOutput
}

// Returns the reason for the test case closure.
func (t *TestCase) Reason() string {
	return t.reason
}

// Returns the error that caused the test case to fail. This is nil if the test
// case did not fail because of an error.
func (t *TestCase) GetError() error {
	return t.err
}

// Mark a pending test as skipped because of a dependency failure.
func (t *TestCase) MarkNotRun(reason string) {
	t.close(TestCaseStatusNotRun, reason, nil)
}

func (t *TestCase) SetCollectedOutput(val []string) {
	t.collectedOutput = val
}

// Mark a test as errored. This is used when the test case panics or returns an
// error.
func (t *TestCase) MarkError(err error) {
	t.close(TestCaseStatusError, "", err)
}

// Retrieves the status of the test case.
func (t *TestCase) Status() TestCaseStatus {
	return t.status
}

// Close this test case as passed!
func (t *TestCase) Pass() {
	t.close(TestCaseStatusPassed, "", nil)
}

// Return the suite-level cleanup functions registred in this test case.
func (t *TestCase) SuiteCleanupList() []func() {
	return t.suiteCleanup
}

// storm.TestCase implementations:

// SuiteCleanup implements core.TestCase.
func (t *TestCase) SuiteCleanup(f func()) {
	t.suiteCleanup = append(t.suiteCleanup, f)
}

// Registrant implements core.TestCase.
func (t *TestCase) Registrant() core.TestRegistrantMetadata {
	return t.registrant
}

// Error implements core.TestCase.
func (t *TestCase) Error(err error) {
	t.close(TestCaseStatusError, "", err)
	runtime.Goexit()
}

// Fail implements core.TestCase.
func (t *TestCase) Fail(reason string) {
	t.close(TestCaseStatusFailed, reason, nil)
	runtime.Goexit()
}

// FailFromError implements core.TestCase.
func (t *TestCase) FailFromError(err error) {
	t.close(TestCaseStatusFailed, "", err)
	runtime.Goexit()
}

// Skip implements core.TestCase.
func (t *TestCase) Skip(reason string) {
	t.close(TestCaseStatusSkipped, reason, nil)
	runtime.Goexit()
}

// Name implements core.TestCase.
func (t *TestCase) Name() string {
	return t.name
}

// RunTime implements core.TestCase.
func (t *TestCase) RunTime() time.Duration {
	return t.endTime.Sub(t.startTime)
}

// Context implements core.TestCase.
func (t *TestCase) Context() context.Context {
	return t.ctx
}

// BackgroundWaitGroup implements core.TestCase.
func (t *TestCase) BackgroundWaitGroup() *sync.WaitGroup {
	return &t.waitGroup
}
