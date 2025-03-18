package reporter

import (
	"fmt"
	"storm/internal/testmgr"
)

type TestSummary struct {
	total   int
	passed  int
	failed  int
	skipped int
	errored int
}

func newSummaryFromTestManager(tm *testmgr.StormTestManager) TestSummary {
	var summary TestSummary

	for _, testCase := range tm.TestCases() {
		summary.total++
		switch testCase.Status() {
		case testmgr.TestCaseStatusPassed:
			summary.passed++
		case testmgr.TestCaseStatusFailed:
			summary.failed++
		case testmgr.TestCaseStatusSkipped:
			summary.skipped++
		case testmgr.TestCaseStatusError:
			summary.errored++
		default:
			panic("Invalid test case status")
		}
	}

	return summary
}

func (s TestSummary) Status() TestSummaryStatus {
	if s.errored > 0 {
		return TestStatusError
	}
	if s.failed > 0 {
		return TestStatusFailed
	}
	return TestStatusOk
}

func (s TestSummary) Summary() string {
	return fmt.Sprintf(
		"total: %d; passed: %d; failed: %d; skipped: %d; errored: %d",
		s.total,
		s.passed,
		s.failed,
		s.skipped,
		s.errored,
	)
}