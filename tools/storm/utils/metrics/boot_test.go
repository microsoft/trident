package metrics

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestParseBootMetricFullBreakdown(t *testing.T) {
	const output = "Startup finished in 13.022s (firmware) + 2.552s (loader) + 4.740s (kernel) + 1.267s (initrd) + 15.249s (userspace) = 35.565s"

	got, err := ParseBootMetric("install", output)
	if err != nil {
		t.Fatalf("ParseBootMetric: %v", err)
	}

	want := BootMetric{
		Operation:   "install",
		FirmwareMs:  13022,
		LoaderMs:    2552,
		KernelMs:    4740,
		InitrdMs:    1267,
		UserspaceMs: 15249,
	}
	if got != want {
		t.Errorf("got %+v, want %+v", got, want)
	}
}

func TestParseBootMetricOmitsAbsentPhases(t *testing.T) {
	// A VM without firmware/loader timings reports only the later phases.
	const output = "Startup finished in 4.740s (kernel) + 1.267s (initrd) + 15.249s (userspace) = 21.256s"

	got, err := ParseBootMetric("update1", output)
	if err != nil {
		t.Fatalf("ParseBootMetric: %v", err)
	}

	if got.FirmwareMs != 0 || got.LoaderMs != 0 {
		t.Errorf("absent phases should stay zero, got %+v", got)
	}
	if got.KernelMs != 4740 || got.Operation != "update1" {
		t.Errorf("unexpected parse result: %+v", got)
	}
}

func TestParseBootMetricNormalizesUnits(t *testing.T) {
	const output = "Startup finished in 1.5m (firmware) + 250ms (loader) + 2s (kernel) = x"

	got, err := ParseBootMetric("install", output)
	if err != nil {
		t.Fatalf("ParseBootMetric: %v", err)
	}

	if got.FirmwareMs != 90000 {
		t.Errorf("minutes not normalized: %v", got.FirmwareMs)
	}
	if got.LoaderMs != 250 {
		t.Errorf("milliseconds not preserved: %v", got.LoaderMs)
	}
	if got.KernelMs != 2000 {
		t.Errorf("seconds not normalized: %v", got.KernelMs)
	}
}

func TestParseBootMetricSumsCompoundDurations(t *testing.T) {
	// systemd-analyze splits long phases into multiple components. The original
	// single-character-unit regex matched only the last one (or nothing at all
	// for a "ms" value), silently recording the phase as zero.
	const output = "Startup finished in 1min 234ms (firmware) + 500ms (initrd) = 1min 734ms"

	got, err := ParseBootMetric("install", output)
	if err != nil {
		t.Fatalf("ParseBootMetric: %v", err)
	}

	if got.FirmwareMs != 60234 {
		t.Errorf("compound duration not summed, got %v want 60234", got.FirmwareMs)
	}
	if got.InitrdMs != 500 {
		t.Errorf("millisecond duration not captured, got %v want 500", got.InitrdMs)
	}
}

func TestAppendBootMetricsInheritsContextFromFirstRecord(t *testing.T) {
	path := filepath.Join(t.TempDir(), "metrics.jsonl")
	const existing = `{"timestamp":"t1","metric_name":"a","value":1,` +
		`"additional_fields":{"trident_config_name":"base"},` +
		`"platform_info":{"machine_type":"virtualMachine"}}` + "\n"
	if err := os.WriteFile(path, []byte(existing), 0o644); err != nil {
		t.Fatalf("write metrics file: %v", err)
	}

	value := BootMetric{Operation: "install", KernelMs: 4740}
	if err := AppendBootMetrics(path, value); err != nil {
		t.Fatalf("AppendBootMetrics: %v", err)
	}

	f, err := os.Open(path)
	if err != nil {
		t.Fatalf("open metrics file: %v", err)
	}
	defer f.Close()

	var records []map[string]any
	dec := json.NewDecoder(f)
	for dec.More() {
		var record map[string]any
		if err := dec.Decode(&record); err != nil {
			t.Fatalf("decode: %v", err)
		}
		records = append(records, record)
	}

	if len(records) != 2 {
		t.Fatalf("expected the record to be appended, got %d records", len(records))
	}

	boot := records[1]
	if boot["metric_name"] != BootMetricName {
		t.Errorf("unexpected metric_name: %v", boot["metric_name"])
	}
	// The boot record must carry the same context as the records Trident
	// reported, otherwise it lands in Kusto without a configuration to join on.
	additional := boot["additional_fields"].(map[string]any)
	if additional["trident_config_name"] != "base" {
		t.Errorf("additional_fields not inherited: %v", additional)
	}
	platform := boot["platform_info"].(map[string]any)
	if platform["machine_type"] != "virtualMachine" {
		t.Errorf("platform_info not inherited: %v", platform)
	}
	if boot["value"].(map[string]any)["kernel"] != float64(4740) {
		t.Errorf("boot value not recorded: %v", boot["value"])
	}
}

func TestAppendBootMetricsFailsOnEmptyFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "metrics.jsonl")
	if err := os.WriteFile(path, nil, 0o644); err != nil {
		t.Fatalf("write metrics file: %v", err)
	}

	if err := AppendBootMetrics(path, BootMetric{Operation: "install"}); err == nil {
		t.Error("expected an error when there is no record to inherit context from")
	}
}

// The boot record must survive enrichment, since the pipeline appends it before
// the enrichment pass runs.
func TestEnrichFileHandlesAnAppendedBootRecord(t *testing.T) {
	path := filepath.Join(t.TempDir(), "metrics.jsonl")
	const existing = `{"timestamp":"t1","metric_name":"a","value":1,"additional_fields":{},"platform_info":{}}` + "\n"
	if err := os.WriteFile(path, []byte(existing), 0o644); err != nil {
		t.Fatalf("write metrics file: %v", err)
	}

	if err := AppendBootMetrics(path, BootMetric{Operation: "install", KernelMs: 1}); err != nil {
		t.Fatalf("AppendBootMetrics: %v", err)
	}

	count, err := EnrichFile(path, NewEnrichment(testPipeline(), nil))
	if err != nil {
		t.Fatalf("EnrichFile: %v", err)
	}
	if count != 2 {
		t.Fatalf("expected both records to be enriched, got %d", count)
	}

	records := readRecords(t, path)
	boot := records[1]
	if boot["metric_name"] != BootMetricName {
		t.Fatalf("expected the boot record last, got %v", boot["metric_name"])
	}
	if boot["platform_info"].(map[string]any)["machine_type"] != "vm" {
		t.Errorf("the boot record was not enriched: %v", boot["platform_info"])
	}
}

// systemd-analyze writes its diagnostics to stderr and produces no usable
// stdout when the boot has not finished. Recording that as an all-zero record
// would be indistinguishable in Kusto from a genuinely instant boot.
func TestParseBootMetricRejectsOutputWithNoPhases(t *testing.T) {
	for name, output := range map[string]string{
		"empty":        "",
		"not finished": "Bootup is not yet finished (kernel is still initializing).",
		"unrelated":    "Failed to get timestamp properties: Transport endpoint is not connected",
	} {
		t.Run(name, func(t *testing.T) {
			if _, err := ParseBootMetric("install", output); !errors.Is(err, ErrNoBootPhases) {
				t.Errorf("got err=%v, want ErrNoBootPhases", err)
			}
		})
	}
}

// systemd-analyze prints per-target lines after the breakdown; only the first
// line carries the phase timings.
func TestParseBootMetricUsesOnlyTheFirstLine(t *testing.T) {
	const output = "Startup finished in 4.740s (kernel) + 15.249s (userspace) = 19.989s\n" +
		"graphical.target reached after 13.272s in userspace\n"

	got, err := ParseBootMetric("install", output)
	if err != nil {
		t.Fatalf("ParseBootMetric: %v", err)
	}
	if got.KernelMs != 4740 || got.UserspaceMs != 15249 {
		t.Errorf("unexpected parse of multi-line output: %+v", got)
	}
}
