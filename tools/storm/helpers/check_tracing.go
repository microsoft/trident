package helpers

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
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
		SyslogMetricToCheck           string `help:"Name of the metric to check for in journald logs. Note this metric is queried after target OS runs commit, so the metric must be emitted by commit." default:"trident_start"`
		MetricFile                    string `help:"Path to the file containing the expected metric value." default:""`
		FeatureUsageMetric            string `help:"Name of the metric to check for in the metric file, which is collected throughout servicing." default:"host_config_feature_usage"`
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
			if errors.Is(err, io.EOF) {
				break
			}
			return fmt.Errorf("failed to decode JSON from metric file: %w", err)
		}

		if logEntry["metric_name"] == h.args.FeatureUsageMetric {
			// Test passed
			logrus.Infof("Found expected metric '%s' in trace file with value: %v", h.args.FeatureUsageMetric, logEntry["value"])
			return nil
		}
	}

	// Test failed
	tc.Fail(fmt.Sprintf("Expected metric '%s' not found in trace file '%s'", h.args.FeatureUsageMetric, h.args.MetricFile))
	return nil
}

func (h *CheckJournaldHelper) checkJournald(tc storm.TestCase) error {
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	tridentJournaldTraceLogs, err := stormsshclient.RunCommand(client, fmt.Sprintf("sudo journalctl -t %s -o json", h.args.SyslogIdentifier))
	if err != nil {
		return err
	}

	collectedLogs := tridentJournaldTraceLogs.Stdout
	logrus.Infof("Journald logs for %s:\n%s\n", h.args.SyslogIdentifier, collectedLogs)

	// Read each line from the collectedLogs string, parse the line as JSON, and check if the expected metric_name is present
	// If found, the test passes.
	// If not found, the test fails.
	lines := strings.Split(collectedLogs, "\n")
	for _, line := range lines {
		var logEntry map[string]interface{}
		if err := json.Unmarshal([]byte(line), &logEntry); err != nil {
			logrus.Warnf("Failed to parse line as JSON: %s, error: %v", line, err)
			continue
		}

		if logEntry["F_METRIC_NAME"] == h.args.SyslogMetricToCheck {
			// Test passed
			logrus.Infof("Found expected metric '%s' in journald logs with value: %v", h.args.SyslogMetricToCheck, logEntry["F_METRIC_VALUE"])
			return nil
		}
	}

	// Metric not found in journald logs, test failed
	tc.Fail(fmt.Sprintf("Expected metric '%s' not found in journald logs for syslog identifier '%s'", h.args.SyslogMetricToCheck, h.args.SyslogIdentifier))
	return nil
}
