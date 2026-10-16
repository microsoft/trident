package tests

import (
	"reflect"
	"strings"
	"testing"
)

func TestNebraskaDiagnosticLines(t *testing.T) {
	expected := []string{
		"agent: Nebraska request to 'https://example.invalid/<redacted>':",
		"agent: Nebraska HTTP response: status=200 headers_elapsed_ms=4 content_length=100",
		"agent: Nebraska response: app 'app-1' status ok, updatecheck ok",
		"agent: Resolved COSI metadata SHA-384: source=labels (cosi.metadata.sha384) metadata_sha384=abcd",
	}
	input := strings.Join([]string{
		expected[0],
		`<request machineid="not-for-pipeline"><app/></request>`,
		expected[1],
		"hyper: connection state changed",
		expected[2],
		expected[3],
		"agent: received node update with unrelated data",
	}, "\n")
	if got := nebraskaDiagnosticLines(input); !reflect.DeepEqual(got, expected) {
		t.Fatalf("unexpected diagnostics: %#v", got)
	}
	if got := nebraskaDiagnosticLines(""); len(got) != 0 {
		t.Fatalf("expected no diagnostics for an empty journal, got %#v", got)
	}
}
