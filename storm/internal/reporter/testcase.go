package reporter

import (
	"bytes"
	"fmt"
	"io"
	"runtime"
	"time"

	"github.com/sirupsen/logrus"
)

type TestCaseStatus int

const (
	TestCaseStatusRunning TestCaseStatus = iota
	TestCaseStatusPassed
	TestCaseStatusFailed
	TestCaseStatusSkipped
	TestCaseStatusError
)

func (tcs TestCaseStatus) String() string {
	switch tcs {
	case TestCaseStatusPassed:
		return "PASS"
	case TestCaseStatusFailed:
		return "FAIL"
	case TestCaseStatusSkipped:
		return "SKIP"
	case TestCaseStatusError:
		return "ERROR"
	default:
		return "UNKNOWN"
	}
}

func (tcs TestCaseStatus) logLevel() logrus.Level {
	switch tcs {
	case TestCaseStatusPassed:
		return logrus.InfoLevel
	case TestCaseStatusFailed:
		return logrus.ErrorLevel
	case TestCaseStatusSkipped:
		return logrus.WarnLevel
	case TestCaseStatusError:
		return logrus.ErrorLevel
	default:
		return logrus.InfoLevel
	}
}

func (tcs TestCaseStatus) shouldStopExecution() bool {
	switch tcs {
	case TestCaseStatusPassed:
		return false
	default:
		return true
	}
}

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

func (tc *TestCase) id() string {
	return fmt.Sprintf("%04d:%s", tc.index, tc.name)
}

func (tc *TestCase) isRunning() bool {
	return tc.status == TestCaseStatusRunning
}

func (tc *TestCase) passed() bool {
	return tc.status == TestCaseStatusPassed
}

func (tc *TestCase) logLines() []string {
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

func (tc *TestCase) close(status TestCaseStatus) {
	if tc.status != TestCaseStatusRunning {
		return
	}

	if status == TestCaseStatusRunning {
		panic("cannot close test case with status running")
	}

	tc.endTime = time.Now()
	tc.status = status
	tc.log.Out = io.Discard

	var level logrus.Level = logrus.InfoLevel
	switch tc.status {
	case TestCaseStatusSkipped:
		level = logrus.WarnLevel
	case TestCaseStatusFailed:
		level = logrus.ErrorLevel
	case TestCaseStatusError:
		level = logrus.ErrorLevel
	}

	tc.parent.suite.Logger().
		WithField("testCase", tc.name).
		WithField("status", tc.status.String()).
		Logf(level, "%s: %s", tc.Name(), tc.status.String())

	if tc.status.shouldStopExecution() {
		tc.parent.suite.Logger().Tracef(
			"Stopping execution of [%s] due to test case status '%s'",
			tc.id(),
			tc.status.String(),
		)
		runtime.Goexit()
	}
}

func (tc *TestCase) Logger() *logrus.Logger {
	return tc.log
}

func (tc *TestCase) Fail(reason string) {
	status := TestCaseStatusFailed
	tc.Logger().WithField("reason", reason).Errorf(status.String())
	tc.close(status)
}

func (tc *TestCase) FailFromError(err error) {
	status := TestCaseStatusFailed
	tc.Logger().WithError(err).Errorf(status.String())
	tc.close(status)
}

func (tc *TestCase) Pass() {
	status := TestCaseStatusPassed
	tc.Logger().Info(status.String())
	tc.close(status)
}

func (tc *TestCase) Error(err error) {
	status := TestCaseStatusError
	tc.Logger().WithError(err).Errorf(status.String())
	tc.close(status)
}

func (tc *TestCase) Skip(reason string) {
	status := TestCaseStatusSkipped
	tc.Logger().WithField("reason", reason).Warnf(status.String())
	tc.close(status)
}

func (tc *TestCase) RunTime() time.Duration {
	if tc.status == TestCaseStatusRunning {
		return time.Since(tc.startTime)
	}

	return tc.endTime.Sub(tc.startTime)
}
