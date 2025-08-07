package reporter

import (
	"fmt"
	"os"
	"path/filepath"
	"storm/internal/devops"
	"storm/internal/stormerror"
	"storm/internal/testmgr"
	"storm/pkg/storm/utils"
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

func (tr *TestReporter) SaveLogs(dir string) error {
	for _, testCase := range tr.testManager.TestCases() {
		filename := fmt.Sprintf("%s.log", testCase.Name())
		filepath := filepath.Join(dir, filename)
		err := saveTestCaseLogs(testCase, filepath)
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to save logs for %s: %v\n", testCase.Name(), err)
		}
	}

	return nil
}

func saveTestCaseLogs(testCase *testmgr.TestCase, path string) error {
	file, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("failed to create log file for %s: %v", testCase.Name(), err)
	}
	defer file.Close()

	logs := testCase.CollectedOutput()
	for _, line := range logs {
		line = strings.TrimSpace(utils.ANSI_CLEANER.ReplaceAllString(line, "")) + "\n"
		_, err = file.WriteString(line)
		if err != nil {
			return fmt.Errorf("failed to write log line for %s: %v", testCase.Name(), err)
		}
	}

	return nil
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
	isDevops := tr.testManager.Suite().AzureDevops()
	header := true
	for _, testCase := range tr.testManager.TestCases() {
		status := testCase.Status()
		if !isDevops && (status.Passed() || status.NotRun()) {
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

		testCaseHeader := fmt.Sprintf(
			"Test case: '%s' status: %s; ",
			testCase.Name(),
			statusStr,
		)

		if reason := testCase.Reason(); reason != "" {
			testCaseHeader += fmt.Sprintf("reason: %s; ", reason)
		}

		var grp *devops.Group = nil
		if isDevops {
			grp = devops.OpenGroup(testCaseHeader)
		} else {
			fmt.Print(testCaseHeader)
		}

		panicked := false
		if err, ok := testCase.GetError().(stormerror.PanicError); ok {
			panicked = true
			fmt.Printf("Stack trace:\n%s\n", err.Stack)
		}

		logLines := testCase.CollectedOutput()

		// Check if there are any log lines
		if len(logLines) == 0 {
			if panicked || isDevops {
				fmt.Println("(No logs were collected)")
			} else {
				fmt.Println("no logs were collected.")
			}
		} else {
			if panicked || isDevops {
				fmt.Println("Collected logs:")
			} else {
				fmt.Println("collected logs:")
			}
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

		if grp != nil {
			grp.Close()
		}
	}
}
