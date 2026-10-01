package validate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"tridenttools/storm/utils/trident"
)

func writeTempFile(t *testing.T, content string) string {
	t.Helper()
	dir := t.TempDir()
	p := filepath.Join(dir, "metrics.jsonl")
	if err := os.WriteFile(p, []byte(content), 0644); err != nil {
		t.Fatalf("write temp file: %v", err)
	}
	return p
}

func TestValidateTraceFileMetric_Found(t *testing.T) {
	// Concatenated JSON objects, matching netlisten's trace-stream format.
	path := writeTempFile(t,
		`{"metric_name":"something_else","value":1}`+"\n"+
			`{"metric_name":"host_config_feature_usage","value":42}`+"\n")
	var sa SoftAsserter
	ValidateTraceFileMetric(&sa, path)
	if sa.HasFailures() {
		t.Errorf("expected pass, got failures: %v", sa.Err())
	}
}

func TestValidateTraceFileMetric_NotFound(t *testing.T) {
	path := writeTempFile(t, `{"metric_name":"other","value":1}`+"\n")
	var sa SoftAsserter
	ValidateTraceFileMetric(&sa, path)
	if !sa.HasFailures() {
		t.Error("expected a failure when the feature-usage metric is absent")
	}
}

func TestValidateTraceFileMetric_EmptyPathSkips(t *testing.T) {
	var sa SoftAsserter
	ValidateTraceFileMetric(&sa, "")
	if sa.HasFailures() || sa.Failures() != 0 {
		t.Error("empty trace file path should be skipped, not failed")
	}
	if len(sa.results) != 1 || sa.results[0].name != "tracing/trace-file" {
		t.Fatalf("empty trace file path should record an explicit skip, got %#v", sa.results)
	}
}

func TestValidateTraceFileMetric_MissingFileFails(t *testing.T) {
	var sa SoftAsserter
	ValidateTraceFileMetric(&sa, filepath.Join(t.TempDir(), "does-not-exist.jsonl"))
	if !sa.HasFailures() {
		t.Error("a configured but missing trace file should fail")
	}
}

func TestValidateTraceFileMetric_EmptyFileFails(t *testing.T) {
	path := writeTempFile(t, "")
	var sa SoftAsserter
	ValidateTraceFileMetric(&sa, path)
	if !sa.HasFailures() {
		t.Error("a configured but empty trace file should fail")
	}
}

func TestValidateSelinuxDenialsForRuntime_ContainerRecordsExplicitSkip(t *testing.T) {
	var sa SoftAsserter
	ValidateSelinuxDenialsForRuntime(&sa, nil, trident.RuntimeTypeContainer)

	if sa.HasFailures() {
		t.Fatalf("container SELinux skip should not fail: %v", sa.Err())
	}
	summary := sa.Summary()
	for _, want := range []string{
		"1 validation sub-check(s): 1 passed, 0 failed",
		"PASS  selinux/denials",
	} {
		if !strings.Contains(summary, want) {
			t.Errorf("Summary() = %q, missing %q", summary, want)
		}
	}
}
