package testmgr

import (
	"bytes"
	"fmt"
	"io"
	"runtime"
	"time"

	"github.com/sirupsen/logrus"
)

type TestCase struct {
	name      string
	index     uint
	parent    *StormTestManager
	startTime time.Time
	endTime   time.Time
	status    TestCaseStatus
	log       *logrus.Logger
	logBuffer bytes.Buffer
}

// Implementer of logrus.Hook interface to tee log messages from the test case
// logger to the suite logger
type testCaseLogTee struct {
	suiteLogger *logrus.Logger
	testCaseId  string
}

func (tee testCaseLogTee) Levels() []logrus.Level {
	return logrus.AllLevels
}

func (tee testCaseLogTee) Fire(entry *logrus.Entry) error {
	// Make a shallow copy so that we can modify the logger pointer
	newEntry := tee.suiteLogger.WithFields(entry.Data)
	newEntry.Caller = entry.Caller
	newEntry.Log(entry.Level, fmt.Sprintf("[%s] > %s", tee.testCaseId, entry.Message))
	return nil
}

func newTestCase(name string, index uint, parent *StormTestManager) *TestCase {
	tc := &TestCase{
		name:      name,
		index:     index,
		parent:    parent,
		startTime: time.Now(),
		status:    TestCaseStatusRunning,
		log:       logrus.New(),
	}

	tc.log.SetLevel(logrus.TraceLevel)
	tc.log.SetOutput(&tc.logBuffer)
	tc.log.SetFormatter(&logrus.TextFormatter{
		ForceColors:      true,
		DisableTimestamp: false,
	})
	tc.log.AddHook(testCaseLogTee{
		suiteLogger: parent.suite.Logger(),
		testCaseId:  tc.id(),
	})
	tc.log.SetReportCaller(true)

	return tc
}

func (tc *TestCase) Status() TestCaseStatus {
	return tc.status
}

func (tc *TestCase) id() string {
	return fmt.Sprintf("%04d:%s", tc.index, tc.name)
}

func (tc *TestCase) isRunning() bool {
	return tc.status == TestCaseStatusRunning
}

func (tc *TestCase) LogLines() []string {
	rawLines := bytes.Split(tc.logBuffer.Bytes(), []byte("\n"))
	lines := make([]string, len(rawLines))
	for i, line := range rawLines {
		lines[i] = string(line)
	}

	return lines
}

func (tc *TestCase) Name() string {
	return tc.name
}

func (tc *TestCase) close(status TestCaseStatus, reason string, err error) {
	if tc.status != TestCaseStatusRunning {
		tc.parent.suite.
			Logger().
			Warnf(
				"Attempted to close test case '%s' with status '%s', but it was already closed with status '%s'. Ignoring.",
				tc.name,
				status.String(),
				tc.status.String(),
			)
		return
	}

	if status == TestCaseStatusRunning {
		panic("cannot close test case with status running")
	}

	tc.status = status
	tc.endTime = time.Now()

	// Log the status to the test case logger
	tc.Logger().SetReportCaller(false)
	localEntry := logrus.NewEntry(tc.log)

	if reason != "" {
		localEntry = localEntry.WithField("reason", reason)
	}

	if err != nil {
		localEntry = localEntry.WithError(err)
	}

	localEntry.Log(tc.status.logLevel(), tc.status.String())

	// Close this logger
	tc.log.Out = io.Discard

	// Log the status to the suite logger
	tc.parent.suite.Logger().
		WithField("testCase", tc.name).
		WithField("status", tc.status.String()).
		Logf(tc.status.logLevel(), "%s: %s", tc.Name(), tc.status.String())
}

func (tc *TestCase) Logger() *logrus.Logger {
	return tc.log
}

func (tc *TestCase) Fail(reason string) {
	tc.close(TestCaseStatusFailed, reason, nil)
	tc.stopTestExecution()
}

func (tc *TestCase) FailFromError(err error) {
	tc.close(TestCaseStatusFailed, "", err)
	tc.stopTestExecution()
}

func (tc *TestCase) Pass() {
	tc.close(TestCaseStatusPassed, "", nil)
}

func (tc *TestCase) Error(err error) {
	tc.close(TestCaseStatusError, "", err)
	tc.stopTestExecution()
}

func (tc *TestCase) Skip(reason string) {
	tc.close(TestCaseStatusSkipped, reason, nil)
	tc.stopTestExecution()
}

func (tc *TestCase) SkipAndContinue(reason string) {
	tc.close(TestCaseStatusSkipped, reason, nil)
}

func (tc *TestCase) RunTime() time.Duration {
	if tc.status == TestCaseStatusRunning {
		return time.Since(tc.startTime)
	}

	return tc.endTime.Sub(tc.startTime)
}

// Used internally to attach an error to this test case when the test case
// manager catches an error.
func (tc *TestCase) markError(err error) {
	tc.close(TestCaseStatusError, "", err)
}

// Used internally to close the test case with a passed status when the test
// case manager is closed and no error was found.
func (tc *TestCase) markPass() {
	tc.close(TestCaseStatusPassed, "", nil)
}

// Calls runtime.Goexit() if the test case status is not passed.
// THIS SHOULD ONLY BE CALLED AFTER CLOSING THE TEST CASE!
func (tc *TestCase) stopTestExecution() {
	if tc.status == TestCaseStatusPassed {
		// A pass should never stop the execution of the test runner!
		return
	}

	if tc.status == TestCaseStatusRunning {
		panic("cannot stop test case execution with status running")
	}

	tc.parent.suite.Logger().Tracef(
		"Stopping execution of [%s] due to test case status '%s'",
		tc.id(),
		tc.status.String(),
	)
	runtime.Goexit()
}
