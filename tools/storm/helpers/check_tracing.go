package helpers

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type CheckJournaldHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		trident.RuntimeCliSettings    `embed:""`
		SyslogIdentifier              string `help:"Syslog identifier to check for in journald logs." default:"trident-tracing"`
		SyslogMetricToCheck           string `help:"Name of the metric to check for in journald logs." default:"trident_start"`
		MetricFile                    string `help:"Path to the file containing the expected metric value." default:""`
		FileMetricToCheck             string `help:"Name of the metric to check for in the metric file." default:"host_config_uefi_fallback_mode"`
	}
}

func (h CheckJournaldHelper) Name() string {
	return "check-tracing"
}

func (h *CheckJournaldHelper) Args() any {
	return &h.args
}

func (h *CheckJournaldHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("check-journald", h.checkJournald)
	r.RegisterTestCase("check-trace-file", h.checkTraceFile)
	return nil
}

func (h *CheckJournaldHelper) checkTraceFile(tc storm.TestCase) error {
	if h.args.MetricFile == "" {
		tc.Skip("No metric file provided, skipping trace file check.")
	}

	// Read each line from the metric file, parse the line as JSON, and check if the expected metric_name is present
	// If found, the test passes.
	// If not found, the test fails.
	file, err := os.Open(h.args.MetricFile)
	if err != nil {
		return fmt.Errorf("failed to open metric file '%s': %w", h.args.MetricFile, err)
	}
	defer file.Close()

	scanner := json.NewDecoder(file)
	for {
		var logEntry map[string]interface{}
		if err := scanner.Decode(&logEntry); err != nil {
			if strings.Contains(err.Error(), "EOF") {
				break
			}
			return fmt.Errorf("failed to decode JSON from metric file: %w", err)
		}

		if logEntry["metric_name"] == h.args.FileMetricToCheck {
			// Test passed
			logrus.Infof("Found expected metric '%s' in trace file with value: %v", h.args.FileMetricToCheck, logEntry["value"])
			return nil
		}
	}

	// Test failed
	tc.Fail(fmt.Sprintf("Expected metric '%s' not found in trace file '%s'", h.args.FileMetricToCheck, h.args.MetricFile))
	return nil
}

func (h *CheckJournaldHelper) checkJournald(tc storm.TestCase) error {
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	tridentJournaldTraceLogs, err := stormsshclient.RunCommand(client, fmt.Sprintf("sudo journalctl -t %s -o json-pretty", h.args.SyslogIdentifier))
	if err != nil {
		return err
	}

	collectedLogs := tridentJournaldTraceLogs.Stdout
	logrus.Infof("Journald logs for %s:\n%s\n", h.args.SyslogIdentifier, collectedLogs)

	// Check if the expected metric is present in the journald logs
	if !strings.Contains(collectedLogs, fmt.Sprintf("\"F_METRIC_NAME\" : \"%s\"", h.args.SyslogMetricToCheck)) {
		tc.Fail(fmt.Sprintf("Expected metric '%s' not found in journald logs for syslog identifier '%s'", h.args.SyslogMetricToCheck, h.args.SyslogIdentifier))
	}

	return nil
}
