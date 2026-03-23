package scenario

import (
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"strconv"
	"time"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

const bootMetricsFile = "boot-metrics.jsonl"

// bootMetric holds the parsed systemd-analyze boot timing data.
type bootMetric struct {
	Operation   string  `json:"operation"`
	FirmwareMs  float64 `json:"firmware,omitempty"`
	LoaderMs    float64 `json:"loader,omitempty"`
	KernelMs    float64 `json:"kernel,omitempty"`
	InitrdMs    float64 `json:"initrd,omitempty"`
	UserspaceMs float64 `json:"userspace,omitempty"`
}

// bootMetricRecord represents a single boot metrics record written to the JSONL file.
type bootMetricRecord struct {
	Timestamp  string     `json:"timestamp"`
	MetricName string     `json:"metric_name"`
	Value      bootMetric `json:"value"`
}

// collectBootMetrics collects systemd-analyze boot timing data from the test
// host via SSH and appends it to the boot-metrics.jsonl file.
func (s *TridentE2EScenario) collectBootMetrics(tc storm.TestCase, operation string) error {
	if s.sshClient == nil {
		tc.Skip("No SSH client available for boot metrics collection")
		return nil
	}

	logrus.Infof("Collecting boot metrics (operation: %s)", operation)

	output, err := runCommand(s.sshClient, "systemd-analyze | head -n 1")
	if err != nil {
		logrus.WithError(err).Warn("Failed to collect boot metrics via systemd-analyze")
		tc.FailFromError(fmt.Errorf("failed to run systemd-analyze: %w", err))
		return nil
	}

	logrus.Infof("systemd-analyze output: %s", output)

	metric := bootMetric{Operation: operation}
	if val, units, ok := parseBootTiming(output, "(firmware)"); ok {
		metric.FirmwareMs, _ = toMilliseconds(val, units)
	}
	if val, units, ok := parseBootTiming(output, "(loader)"); ok {
		metric.LoaderMs, _ = toMilliseconds(val, units)
	}
	if val, units, ok := parseBootTiming(output, "(kernel)"); ok {
		metric.KernelMs, _ = toMilliseconds(val, units)
	}
	if val, units, ok := parseBootTiming(output, "(initrd)"); ok {
		metric.InitrdMs, _ = toMilliseconds(val, units)
	}
	if val, units, ok := parseBootTiming(output, "(userspace)"); ok {
		metric.UserspaceMs, _ = toMilliseconds(val, units)
	}

	record := bootMetricRecord{
		Timestamp:  time.Now().Format(time.RFC3339),
		MetricName: "boot_info",
		Value:      metric,
	}

	jsonBytes, err := json.Marshal(record)
	if err != nil {
		return fmt.Errorf("failed to marshal boot metrics: %w", err)
	}

	file, err := os.OpenFile(bootMetricsFile, os.O_APPEND|os.O_WRONLY|os.O_CREATE, 0600)
	if err != nil {
		return fmt.Errorf("failed to open boot metrics file: %w", err)
	}
	defer file.Close()

	if _, err := file.WriteString(string(jsonBytes) + "\n"); err != nil {
		return fmt.Errorf("failed to write boot metrics: %w", err)
	}

	logrus.Infof("Boot metrics collected: firmware=%.0fms, kernel=%.0fms, initrd=%.0fms, userspace=%.0fms",
		metric.FirmwareMs, metric.KernelMs, metric.InitrdMs, metric.UserspaceMs)

	return nil
}

// collectInstallBootMetrics collects boot metrics after the initial OS installation.
func (s *TridentE2EScenario) collectInstallBootMetrics(tc storm.TestCase) error {
	return s.collectBootMetrics(tc, "clean-install")
}

// parseBootTiming extracts the numeric value and unit preceding a target string
// in systemd-analyze output.
func parseBootTiming(text, target string) (string, string, bool) {
	re := regexp.MustCompile(`([-+]?\d*\.?\d+)(.)\s+` + regexp.QuoteMeta(target))
	match := re.FindStringSubmatch(text)
	if len(match) >= 3 {
		return match[1], match[2], true
	}
	return "", "", false
}

// toMilliseconds converts a time value with a unit suffix to milliseconds.
func toMilliseconds(value string, unit string) (float64, error) {
	v, err := strconv.ParseFloat(value, 64)
	if err != nil {
		return 0, fmt.Errorf("failed to parse value: %s", value)
	}

	switch unit {
	case "s":
		return v * 1000, nil
	case "m":
		return v * 60 * 1000, nil
	case "ms":
		return v, nil
	case "ns":
		return v / 1000000, nil
	default:
		return 0, fmt.Errorf("unknown time unit: %s", unit)
	}
}
