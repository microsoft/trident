package phonehome

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
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

// The HTTP server runs handlers concurrently and the trace file is one JSON
// record per line, so simultaneous posts must not interleave mid-line. Run
// under -race this also catches unsynchronised access to the shared file.
func TestConcurrentTraceWritesProduceWholeRecords(t *testing.T) {
	dir := t.TempDir()
	tracePath := filepath.Join(dir, "trace.jsonl")

	mux := http.NewServeMux()
	traceFile, err := SetupTraceStream(mux, tracePath)
	if err != nil {
		t.Fatalf("setup tracestream: %v", err)
	}

	server := httptest.NewServer(mux)
	defer server.Close()

	const writers, perWriter = 8, 25
	var wg sync.WaitGroup
	for w := 0; w < writers; w++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for i := 0; i < perWriter; i++ {
				body := fmt.Sprintf(`{"timestamp":"t","metric_name":"m%d_%d","value":1,"additional_fields":{},"platform_info":{}}`, id, i)
				resp, err := http.Post(server.URL+"/tracestream", "application/json", strings.NewReader(body))
				if err != nil {
					t.Errorf("post: %v", err)
					return
				}
				resp.Body.Close()
			}
		}(w)
	}
	wg.Wait()

	if err := traceFile.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}

	// Every line must be a complete JSON object, and none may be lost.
	data, err := os.ReadFile(tracePath)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	lines := strings.Split(strings.TrimSpace(string(data)), "\n")
	if len(lines) != writers*perWriter {
		t.Errorf("got %d records, want %d", len(lines), writers*perWriter)
	}
	for i, line := range lines {
		var rec map[string]any
		if err := json.Unmarshal([]byte(line), &rec); err != nil {
			t.Fatalf("record %d is not whole (interleaved write): %v\n%q", i+1, err, line)
		}
	}
}

// With tracing disabled SetupTraceStream returns before registering a handler,
// so the nil file it returns is never reachable from one. This pins that: the
// route is absent rather than present-and-nil-guarded, which is why the write
// path needs no nil check.
func TestTraceStreamDisabledRegistersNoHandler(t *testing.T) {
	mux := http.NewServeMux()

	traceFile, err := SetupTraceStream(mux, "")
	if err != nil {
		t.Fatalf("expected no error with tracing disabled, got %v", err)
	}
	if traceFile != nil {
		t.Fatalf("expected a nil trace file with tracing disabled, got %v", traceFile)
	}

	if _, pattern := mux.Handler(httptest.NewRequest(http.MethodPost, "/tracestream", nil)); pattern != "" {
		t.Fatalf("expected no /tracestream handler with tracing disabled, got pattern %q", pattern)
	}

	// A caller that posts anyway gets a 404 from the mux; it never reaches a
	// write against the nil file.
	server := httptest.NewServer(mux)
	defer server.Close()

	resp, err := http.Post(server.URL+"/tracestream", "application/json", strings.NewReader(`{"metric_name":"m","value":1}`))
	if err != nil {
		t.Fatalf("request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("expected 404 with tracing disabled, got %d", resp.StatusCode)
	}
}
