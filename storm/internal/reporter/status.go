package reporter

import (
	"github.com/fatih/color"
)

type TestSummaryStatus int

const (
	TestStatusOk TestSummaryStatus = iota
	TestStatusFailed
	TestStatusError
)

func (ts TestSummaryStatus) String() string {
	switch ts {
	case TestStatusOk:
		return "OK"
	case TestStatusFailed:
		return "FAILED"
	case TestStatusError:
		return "ERROR"
	default:
		return "UNKNOWN"
	}
}

func (ts TestSummaryStatus) StringColor() string {
	switch ts {
	case TestStatusOk:
		return color.GreenString(ts.String())
	case TestStatusFailed:
		return color.RedString(ts.String())
	case TestStatusError:
		return color.New(color.FgRed, color.Bold).Sprint(ts.String())
	default:
		return ts.String()
	}
}

func (ts TestSummaryStatus) IsBad() bool {
	return ts == TestStatusFailed || ts == TestStatusError
}
