package reporter

import (
	"fmt"
	"storm/internal/testmgr"
	"strings"
)

type TestSummary struct {
	total   int
	passed  int
	failed  int
	skipped int
	notRun  int
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
		case testmgr.TestCaseStatusNotRun:
			summary.notRun++
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
	var out []string

	if s.failed > 0 {
		out = append(out, fmt.Sprintf("failed: %d", s.failed))
	}

	if s.errored > 0 {
		out = append(out, fmt.Sprintf("errored: %d", s.errored))
	}
	if s.skipped > 0 {
		out = append(out, fmt.Sprintf("skipped: %d", s.skipped))
	}
	if s.notRun > 0 {
		out = append(out, fmt.Sprintf("notrun: %d", s.notRun))
	}

	out = append(out, fmt.Sprintf("passed: %d", s.passed))
	out = append(out, fmt.Sprintf("total: %d", s.total))

	return strings.Join(out, "; ")
}
