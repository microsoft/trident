package reporter

import (
	"bytes"
	"fmt"
	"storm/internal/devops"
	"storm/internal/testmgr"
	"strings"

	"github.com/fatih/color"
)

type TestReporter struct {
	summary     TestSummary
	testManager *testmgr.StormTestManager
	colorize    bool
}

func NewTestReporter(testManager *testmgr.StormTestManager) *TestReporter {
	// Force colors :D
	color.NoColor = false
	return &TestReporter{
		summary:     newSummaryFromTestManager(testManager),
		testManager: testManager,
		colorize:    true,
	}
}

func (tr *TestReporter) Summary() TestSummary {
	return tr.summary
}

func (tr *TestReporter) PrintReport() {
	tr.printShortReport()
	tr.printFailureReport()
	tr.printFinalResult()
}

func (tr *TestReporter) ExitError() error {
	if tr.summary.Status() == TestStatusOk {
		return nil
	}

	return fmt.Errorf("%s:%s: %s",
		tr.testManager.Registrant().RegistrantType().String(),
		tr.testManager.Registrant().Name(),
		tr.summary.Status().String(),
	)
}

// Print a simple list of all test cases and their status.
func (tr *TestReporter) printShortReport() {
	printSeparatorWithTitle(fmt.Sprintf(
		"SUMMARY of %s::%s::%s",
		tr.testManager.Suite().Name(),
		tr.testManager.Registrant().RegistrantType().String(),
		tr.testManager.Registrant().Name(),
	))

	ljust := 0
	// Find the longest test case name
	for _, testCase := range tr.testManager.TestCases() {
		if len(testCase.Name()) > ljust {
			ljust = len(testCase.Name())
		}
	}

	for _, testCase := range tr.testManager.TestCases() {
		statusStr := testCase.Status().String()
		if tr.colorize {
			statusStr = testCase.Status().ColorString()
		}

		spaces := strings.Repeat(".", max(ljust-len(testCase.Name()), 0))

		fmt.Printf(
			"  %s%s: %s",
			testCase.Name(),
			spaces,
			statusStr,
		)

		reason := testCase.Reason()
		if reason != "" {
			if len(reason) > 40 {
				reason = reason[:40] + "..."
			}

			fmt.Printf(" (%s)", reason)
		}

		fmt.Println()
	}

	// Logs devops messages in a separate section
	if tr.testManager.Suite().AzureDevops() && tr.summary.Status().IsBad() {
		printSeparatorWithTitle("DEVOPS LOG")
		for _, testCase := range tr.testManager.TestCases() {
			status := testCase.Status()
			if !status.IsBad() {
				continue
			}
			devops.LogError("%s::%s::%s::%s -> %s (%s)",
				tr.testManager.Suite().Name(),
				tr.testManager.Registrant().RegistrantType().String(),
				tr.testManager.Registrant().Name(),
				testCase.Name(),
				status.String(),
				testCase.Reason(),
			)
		}
	}

}

func (tr *TestReporter) printFinalResult() {
	statusStr := tr.summary.Status().String()
	if tr.colorize {
		statusStr = tr.summary.Status().StringColor()
	}

	printSeparatorWithTitle("RESULT")
	fmt.Printf("%s: %s\n", statusStr, tr.summary.Summary())
}

func (tr *TestReporter) printFailureReport() {
	header := true
	for _, testCase := range tr.testManager.TestCases() {
		status := testCase.Status()
		if status.Passed() || status.NotRun() {
			continue
		}

		if header {
			printSeparatorWithTitle("FAILURE REPORT")
			header = false
		} else {
			printSeparatorChar("-")
		}

		statusStr := testCase.Status().String()
		if tr.colorize {
			statusStr = testCase.Status().ColorString()
		}

		reason := testCase.Reason()

		fmt.Printf(
			"Test case: '%s' status: %s; reason: \"%s\"; ",
			testCase.Name(),
			statusStr,
			reason,
		)

		logLines := getLogLinesFromTestCase(testCase)

		// Check if there are any log lines
		if len(logLines) == 0 {
			fmt.Println("no logs were collected.")
		} else {
			fmt.Println("collected logs:")
		}

		for _, log := range logLines {
			lines := simpleWordWrap(log, termWidth()-8)
			for i, line := range lines {
				if i == 0 {
					fmt.Printf("    ")
				} else {
					fmt.Printf("        ")
				}

				fmt.Println(line)
			}
		}
	}
}

func getLogLinesFromTestCase(testCase *testmgr.TestCase) []string {
	lines := make([]string, 0)
	rawLines := testCase.Buffer().Bytes()
	for _, line := range bytes.Split(rawLines, []byte("\n")) {
		if len(line) == 0 {
			continue
		}
		lines = append(lines, string(line))
	}

	return lines
}
