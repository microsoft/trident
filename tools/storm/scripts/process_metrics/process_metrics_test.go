package process_metrics

import (
	"os"
	"path/filepath"
	"testing"
)

func TestResolveMetricsFilesPrefersExplicitFiles(t *testing.T) {
	h := &ProcessMetricsScript{}
	h.MetricsFiles = []string{"/a.jsonl", "/b.jsonl"}
	h.MetricsDir = t.TempDir()

	got, err := h.resolveMetricsFiles()
	if err != nil {
		t.Fatalf("resolveMetricsFiles: %v", err)
	}
	if len(got) != 2 || got[0] != "/a.jsonl" {
		t.Errorf("explicit files should win, got %v", got)
	}
}

// The clean install is always expected, so it is listed even when absent: that
// is what lets the presence assertion catch a run that should have reported and
// did not. Per-operation files vary by configuration, so only existing ones are
// listed - but empty ones are kept, since an operation that ran without
// reporting is exactly what the assertion exists to catch.
func TestResolveMetricsFilesDiscovers(t *testing.T) {
	dir := t.TempDir()
	write := func(name, content string) {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644); err != nil {
			t.Fatalf("write %s: %v", name, err)
		}
	}
	write("metrics-ab-update-1-ab-update.jsonl", "{}\n")
	write("metrics-manual-rollback.jsonl", "") // empty: must still be listed
	write("unrelated.jsonl", "{}\n")           // must be ignored
	write("metrics-notes.txt", "{}\n")         // wrong extension, ignored

	h := &ProcessMetricsScript{}
	h.MetricsDir = dir

	got, err := h.resolveMetricsFiles()
	if err != nil {
		t.Fatalf("resolveMetricsFiles: %v", err)
	}

	want := []string{
		filepath.Join(dir, "trident-clean-install-metrics.jsonl"), // absent, still expected
		filepath.Join(dir, "metrics-ab-update-1-ab-update.jsonl"),
		filepath.Join(dir, "metrics-manual-rollback.jsonl"),
	}
	if len(got) != len(want) {
		t.Fatalf("got %d files %v, want %d %v", len(got), got, len(want), want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("file %d: got %s, want %s", i, got[i], want[i])
		}
	}
}

func TestResolveMetricsFilesRequiresAnInput(t *testing.T) {
	h := &ProcessMetricsScript{}
	if _, err := h.resolveMetricsFiles(); err == nil {
		t.Error("expected an error when neither --metrics-file nor --metrics-dir is given")
	}
}

// Legacy asserted that expected metrics were present, but only when nothing
// else had failed; otherwise missing metrics are a symptom, not the cause.
func TestJobAlreadyFailed(t *testing.T) {
	for status, want := range map[string]bool{
		"Succeeded":           false,
		"succeeded":           false, // ADO casing varies
		"SucceededWithIssues": true,
		"Failed":              true,
		"Canceled":            true,
		"":                    true, // unset is not a success
	} {
		h := &ProcessMetricsScript{}
		h.JobStatus = status
		if got := h.jobAlreadyFailed(); got != want {
			t.Errorf("status %q: got %v, want %v", status, got, want)
		}
	}
}

func TestBoolVariableUsesAdoCasing(t *testing.T) {
	if boolVariable(true) != "True" || boolVariable(false) != "False" {
		t.Error("Azure DevOps compares these as the literals True/False")
	}
}
