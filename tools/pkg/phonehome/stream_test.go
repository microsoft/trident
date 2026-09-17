package phonehome

import (
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

const malformedRequestHelperEnv = "PHONEHOME_MALFORMED_REQUEST_HELPER"

func TestMalformedStreamRequestsDoNotExitProcess(t *testing.T) {
	if os.Getenv(malformedRequestHelperEnv) == "1" {
		runMalformedStreamRequestHelper(t)
		return
	}

	dir := filepath.Join("test-output", "phonehome-malformed-"+strconv.Itoa(os.Getpid()))
	if err := os.RemoveAll(dir); err != nil {
		t.Fatalf("remove old test output: %v", err)
	}
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatalf("create test output: %v", err)
	}
	defer os.RemoveAll(dir)

	cmd := exec.Command(os.Args[0], "-test.run=^TestMalformedStreamRequestsDoNotExitProcess$")
	cmd.Env = append(os.Environ(),
		malformedRequestHelperEnv+"=1",
		"PHONEHOME_MALFORMED_REQUEST_DIR="+dir,
	)

	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("helper exited unexpectedly; Fatal in a handler would produce this failure: %v\n%s", err, output)
	}
}

func runMalformedStreamRequestHelper(t *testing.T) {
	dir := os.Getenv("PHONEHOME_MALFORMED_REQUEST_DIR")
	if dir == "" {
		t.Fatal("PHONEHOME_MALFORMED_REQUEST_DIR is required")
	}

	if file, err := SetupLogstream(http.NewServeMux(), filepath.Join(dir, "missing", "logstream.log")); err == nil {
		file.Close()
		t.Fatal("expected setup to return the background log creation error")
	}

	mux := http.NewServeMux()
	logFile, err := SetupLogstream(mux, filepath.Join(dir, "logstream.log"))
	if err != nil {
		t.Fatalf("setup logstream: %v", err)
	}
	defer logFile.Close()

	result := make(chan PhoneHomeResult, 1)
	traceFile, err := SetupTraceStream(mux, filepath.Join(dir, "trace.jsonl"), result)
	if err != nil {
		t.Fatalf("setup tracestream: %v", err)
	}

	server := httptest.NewServer(mux)
	defer server.Close()

	assertPostStatus(t, server.URL+"/logstream", `{"message":`, http.StatusBadRequest)
	assertPostStatus(t, server.URL+"/tracestream", `{"metric_name":`, http.StatusBadRequest)

	if err := traceFile.Close(); err != nil {
		t.Fatalf("close trace file: %v", err)
	}

	validTrace := `{"timestamp":"now","metric_name":"boot","value":1,"additional_fields":{},"platform_info":{}}`
	assertPostStatus(t, server.URL+"/tracestream", validTrace, http.StatusInternalServerError)

	select {
	case got := <-result:
		if got.State != PhoneHomeResultError {
			t.Fatalf("trace write failure state = %q, want %q", got.State, PhoneHomeResultError)
		}
		if !strings.Contains(got.Message, "failed to write trace data to file") {
			t.Fatalf("trace write failure message = %q", got.Message)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("trace write failure was not reported through the result channel")
	}
}

func assertPostStatus(t *testing.T, url string, body string, want int) {
	t.Helper()

	resp, err := http.Post(url, "application/json", strings.NewReader(body))
	if err != nil {
		t.Fatalf("post %s: %v", url, err)
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, resp.Body)

	if resp.StatusCode != want {
		t.Fatalf("post %s status = %d, want %d", url, resp.StatusCode, want)
	}
}
