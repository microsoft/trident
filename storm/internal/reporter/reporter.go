package reporter

import (
	"fmt"
	"storm/internal/devops"
	"storm/internal/testmgr"

	"github.com/fatih/color"
	"github.com/sirupsen/logrus"
)

type TestReporter struct {
	summary     TestSummary
	testManager *testmgr.StormTestManager
	devops      bool
	log         *logrus.Logger
	colorize    bool
}

func NewTestReporter(testManager *testmgr.StormTestManager) *TestReporter {
	// Force colors :D
	color.NoColor = false
	return &TestReporter{
		summary:     newSummaryFromTestManager(testManager),
		testManager: testManager,
		devops:      testManager.Suite().AzureDevops(),
		log:         testManager.Suite().Logger(),
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
		tr.testManager.Runnable().RunnableType().String(),
		tr.testManager.Runnable().Name(),
		tr.summary.Status().String(),
	)
}

// Print a simple list of all test cases and their status.
func (tr *TestReporter) printShortReport() {
	printSeparatorWithTitle(fmt.Sprintf(
		"SUMMARY of %s::%s::%s",
		tr.testManager.Suite().Name(),
		tr.testManager.Runnable().RunnableType().String(),
		tr.testManager.Runnable().Name(),
	))

	for _, testCase := range tr.testManager.TestCases() {
		status := testCase.Status()
		statusStr := testCase.Status().String()
		if tr.colorize {
			statusStr = testCaseStatusColor(status)
		}

		fmt.Printf(
			"  %s: %s\n",
			testCase.Name(),
			statusStr,
		)
	}

	// Logs devops messages in a separate section
	if tr.devops && tr.summary.Status().IsBad() {
		printSeparatorWithTitle("DEVOPS LOG")
		for _, testCase := range tr.testManager.TestCases() {
			status := testCase.Status()
			if !status.IsBad() {
				continue
			}
			devops.LogError("%s::%s::%s::%s -> %s",
				tr.testManager.Suite().Name(),
				tr.testManager.Runnable().RunnableType().String(),
				tr.testManager.Runnable().Name(),
				testCase.Name(),
				status.String(),
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
	for i, testCase := range tr.testManager.TestCases() {
		status := testCase.Status()
		if status.Passed() {
			continue
		}

		if i == 0 {
			printSeparatorWithTitle("FAILURE REPORT")
		} else {
			printSeparator()
		}

		statusStr := testCase.Status().String()
		if tr.colorize {
			statusStr = testCaseStatusColor(status)
		}

		fmt.Printf(
			"Test case: '%s' status: %s; collected logs:\n",
			testCase.Name(),
			statusStr,
		)

		for _, log := range testCase.LogLines() {
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
