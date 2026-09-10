package metrics

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// BootMetricName is the metric_name under which boot timings are ingested.
const BootMetricName = "boot_info"

// SystemdAnalyzeCommand returns the first line of systemd-analyze, which is
// where the boot phase breakdown lives.
const SystemdAnalyzeCommand = "systemd-analyze | head -n 1"

// BootMetric is the per-phase boot timing breakdown, in milliseconds.
type BootMetric struct {
	Operation   string  `json:"operation"`
	FirmwareMs  float64 `json:"firmware,omitempty"`
	LoaderMs    float64 `json:"loader,omitempty"`
	KernelMs    float64 `json:"kernel,omitempty"`
	InitrdMs    float64 `json:"initrd,omitempty"`
	UserspaceMs float64 `json:"userspace,omitempty"`
}

// BootMetrics is a boot timing record in the trace-stream schema, so it can be
// appended to the same file Trident's tracestream produced and ingested by the
// same Kusto mapping.
type BootMetrics struct {
	Timestamp        string         `json:"timestamp"`
	MetricName       string         `json:"metric_name"`
	Value            BootMetric     `json:"value"`
	AdditionalFields map[string]any `json:"additional_fields"`
	PlatformInfo     map[string]any `json:"platform_info"`
}

// bootPhases maps each systemd-analyze phase label to where its duration lands
// on a BootMetric. Not every phase is reported by every platform (a VM without
// firmware timings, for instance), so a missing phase is left at zero and
// omitted from the record.
var bootPhases = []struct {
	label string
	set   func(*BootMetric, float64)
}{
	{"(firmware)", func(b *BootMetric, v float64) { b.FirmwareMs = v }},
	{"(loader)", func(b *BootMetric, v float64) { b.LoaderMs = v }},
	{"(kernel)", func(b *BootMetric, v float64) { b.KernelMs = v }},
	{"(initrd)", func(b *BootMetric, v float64) { b.InitrdMs = v }},
	{"(userspace)", func(b *BootMetric, v float64) { b.UserspaceMs = v }},
}

// ParseBootMetric extracts the boot phase timings from the first line of
// systemd-analyze output, which looks like:
//
//	Startup finished in 13.022s (firmware) + 2.552s (loader) + 4.740s (kernel) + 1.267s (initrd) + 15.249s (userspace) = 35.565s
func ParseBootMetric(operation string, systemdAnalyzeOutput string) (BootMetric, error) {
	result := BootMetric{Operation: operation}

	for _, phase := range bootPhases {
		ms, found, err := findDurationBefore(systemdAnalyzeOutput, phase.label)
		if err != nil {
			return result, fmt.Errorf("failed to parse the %s boot phase: %w", phase.label, err)
		}
		if !found {
			continue
		}
		phase.set(&result, ms)
	}

	return result, nil
}

// AppendBootMetrics appends a boot timing record to a trace-stream metrics
// file, inheriting additional_fields and platform_info from the file's first
// record so the boot record carries the same context as the ones Trident
// reported.
func AppendBootMetrics(metricsFile string, value BootMetric) error {
	record, err := newBootMetricsRecord(metricsFile, value)
	if err != nil {
		return err
	}

	encoded, err := json.Marshal(record)
	if err != nil {
		return fmt.Errorf("failed to encode boot metrics: %w", err)
	}

	file, err := os.OpenFile(metricsFile, os.O_APPEND|os.O_WRONLY|os.O_CREATE, 0600)
	if err != nil {
		return fmt.Errorf("failed to open metrics file '%s': %w", metricsFile, err)
	}
	defer file.Close()

	if _, err := file.WriteString(string(encoded) + "\n"); err != nil {
		return fmt.Errorf("failed to append boot metrics to '%s': %w", metricsFile, err)
	}
	return nil
}

// newBootMetricsRecord builds the record to append, seeding its dynamic fields
// from the first record already in the file.
func newBootMetricsRecord(metricsFile string, value BootMetric) (BootMetrics, error) {
	record := BootMetrics{
		Timestamp:  time.Now().Format(time.RFC3339),
		MetricName: BootMetricName,
		Value:      value,
	}

	file, err := os.Open(metricsFile)
	if err != nil {
		return record, fmt.Errorf("failed to open metrics file '%s': %w", metricsFile, err)
	}
	defer file.Close()

	scanner := bufio.NewScanner(file)
	if !scanner.Scan() {
		return record, fmt.Errorf("metrics file '%s' is empty, so there is no context to inherit", metricsFile)
	}

	var first map[string]any
	if err := json.NewDecoder(strings.NewReader(scanner.Text())).Decode(&first); err != nil {
		return record, fmt.Errorf("failed to decode the first record of '%s': %w", metricsFile, err)
	}

	if additional, ok := first[additionalFieldsKey].(map[string]any); ok {
		record.AdditionalFields = additional
	}
	if platform, ok := first[platformInfoKey].(map[string]any); ok {
		record.PlatformInfo = platform
	}

	return record, nil
}

// phaseDurationPattern matches the duration immediately preceding a phase
// label, e.g. "4.740s (kernel)" or "1min 234ms (initrd)". systemd-analyze
// reports fast phases in ms and splits long ones into multiple components, so
// the duration is matched as one-or-more <number><unit> terms and summed.
const phaseDurationPattern = `((?:[-+]?\d*\.?\d+[a-z]+\s*)+)`

// findDurationBefore returns the total duration, in milliseconds, reported
// immediately before a phase label.
func findDurationBefore(text string, target string) (float64, bool, error) {
	re := regexp.MustCompile(phaseDurationPattern + regexp.QuoteMeta(target))
	match := re.FindStringSubmatch(text)
	if len(match) < 2 {
		return 0, false, nil
	}

	var total float64
	for _, term := range durationTermPattern.FindAllStringSubmatch(match[1], -1) {
		ms, err := toMilliseconds(term[1], term[2])
		if err != nil {
			return 0, true, err
		}
		total += ms
	}
	return total, true, nil
}

var durationTermPattern = regexp.MustCompile(`([-+]?\d*\.?\d+)([a-z]+)`)

// toMilliseconds normalizes a systemd-analyze duration to milliseconds, which
// is the unit the Kusto table records.
func toMilliseconds(value string, unit string) (float64, error) {
	parsed, err := strconv.ParseFloat(value, 64)
	if err != nil {
		return 0, fmt.Errorf("failed to parse value '%s': %w", value, err)
	}

	switch unit {
	case "ns":
		return parsed / 1000000, nil
	case "us":
		return parsed / 1000, nil
	case "ms":
		return parsed, nil
	case "s":
		return parsed * 1000, nil
	// systemd-analyze spells minutes "min"; "m" is accepted too since the
	// original implementation did.
	case "m", "min":
		return parsed * 60 * 1000, nil
	case "h":
		return parsed * 60 * 60 * 1000, nil
	}

	return 0, fmt.Errorf("unknown time unit: %s", unit)
}
