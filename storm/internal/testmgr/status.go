package testmgr

import "github.com/fatih/color"

type TestCaseStatus int

const (
	TestCaseStatusPending TestCaseStatus = iota
	TestCaseStatusRunning
	TestCaseStatusPassed
	TestCaseStatusFailed
	TestCaseStatusSkipped
	TestCaseStatusNotRun
	TestCaseStatusError
)

// IsFinal returns true for all test case statuses that are considered final states.
// This includes Passed, Failed, Skipped, and Error statuses.
// It does not include Pending or Running statuses.
func (tcs TestCaseStatus) IsFinal() bool {
	return tcs == TestCaseStatusPassed ||
		tcs == TestCaseStatusFailed ||
		tcs == TestCaseStatusSkipped ||
		tcs == TestCaseStatusNotRun ||
		tcs == TestCaseStatusError
}

func (tcs TestCaseStatus) String() string {
	switch tcs {
	case TestCaseStatusPending:
		return "PEND"
	case TestCaseStatusPassed:
		return "PASS"
	case TestCaseStatusFailed:
		return "FAIL"
	case TestCaseStatusSkipped:
		return "SKIP"
	case TestCaseStatusNotRun:
		return "NOTR"
	case TestCaseStatusError:
		return "ERRO"
	default:
		return "UNKNOWN"
	}
}

func (tcs TestCaseStatus) ColorString() string {
	color.NoColor = false // Force colors
	switch tcs {
	case TestCaseStatusPassed:
		return color.GreenString(tcs.String())
	case TestCaseStatusFailed:
		return color.RedString(tcs.String())
	case TestCaseStatusSkipped:
		return color.YellowString(tcs.String())
	case TestCaseStatusError:
		return color.New(color.FgRed, color.Bold).Sprint(tcs.String())
	default:
		return tcs.String()
	}
}

func (tcs TestCaseStatus) IsPending() bool {
	return tcs == TestCaseStatusPending
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

func (tcs TestCaseStatus) NotRun() bool {
	return tcs == TestCaseStatusNotRun
}

// IsBad returns true if the test case status is either Failed or Error.
func (tcs TestCaseStatus) IsBad() bool {
	return tcs == TestCaseStatusFailed || tcs == TestCaseStatusError
}
