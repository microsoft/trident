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

	traceFile, err := SetupTraceStream(mux, filepath.Join(dir, "trace.jsonl"))
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

	// The 500 above is the whole contract for a trace write failure: it is
	// logged and answered, and the process is still alive to answer it.
	//
	// It deliberately does NOT reach the phone-home result channel. That
	// channel carries the servicing outcome, and netlisten's ListenLoop runs
	// with waitForProvisioned=false, so any non-Failure result there makes it
	// return immediately -- aborting a servicing run because its telemetry
	// could not be written. A missing metric is caught later by the suite's
	// trace-file validation instead, which is the check actually about
	// telemetry.
	assertPostStatus(t, server.URL+"/logstream", `{"message":"still alive"}`, http.StatusCreated)
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
