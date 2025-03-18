package testmgr

import "github.com/sirupsen/logrus"

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

func (tcs TestCaseStatus) IsRunning() bool {
	return tcs == TestCaseStatusRunning
}

func (tcs TestCaseStatus) Passed() bool {
	return tcs == TestCaseStatusPassed
}

func (tcs TestCaseStatus) Failed() bool {
	return tcs == TestCaseStatusFailed
}

func (tcs TestCaseStatus) Skipped() bool {
	return tcs == TestCaseStatusSkipped
}

func (tcs TestCaseStatus) Errored() bool {
	return tcs == TestCaseStatusError
}

// IsBad returns true if the test case status is either Failed or Error.
func (tcs TestCaseStatus) IsBad() bool {
	return tcs == TestCaseStatusFailed || tcs == TestCaseStatusError
}
