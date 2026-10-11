package phonehome

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"sync"

	uuid "github.com/google/uuid"
	log "github.com/sirupsen/logrus"
)

type TraceEntry struct {
	Timestamp        string                 `json:"timestamp"`
	MetricName       string                 `json:"metric_name"`
	Value            interface{}            `json:"value"`
	AdditionalFields map[string]interface{} `json:"additional_fields"`
	PlatformInfo     map[string]interface{} `json:"platform_info"`
}

func SetupTraceStream(mux *http.ServeMux, filepath string) (*os.File, error) {
	if filepath == "" {
		return nil, nil
	}

	if mux == nil {
		mux = http.DefaultServeMux
	}

	// Setup a file to store the trace data
	traceFile, err := os.Create(filepath)
	if err != nil {
		return nil, fmt.Errorf("failed to create trace file: %w", err)
	}

	// Generate a UUID to group events coming in from the same Trident run
	traceID := uuid.New().String()

	// The HTTP server runs handlers concurrently. Record integrity does not
	// actually depend on this lock -- os.File serialises writes internally
	// (internal/poll.FD.Write takes a write lock), so a WriteString of a whole
	// line cannot interleave with another, and a 200-writer test passes under
	// -race without it.
	//
	// It is held anyway so the write and the Sync that follows behave as one
	// unit, and so the invariant this file depends on -- one whole JSON record
	// per line -- is stated in the code rather than inherited from a detail of
	// the standard library.
	var writeMu sync.Mutex

	mux.HandleFunc("/tracestream", func(w http.ResponseWriter, r *http.Request) {
		var traceEntry TraceEntry
		err := json.NewDecoder(r.Body).Decode(&traceEntry)
		if err != nil {
			log.WithError(err).Error("failed to decode trace entry")
			http.Error(w, "failed to decode trace entry", http.StatusBadRequest)
			return
		}

		if traceEntry.AdditionalFields == nil {
			traceEntry.AdditionalFields = map[string]interface{}{}
		}
		traceEntry.AdditionalFields["trace_id"] = traceID

		// write the trace data as json
		traceData, err := json.Marshal(traceEntry)
		if err != nil {
			log.WithError(err).Error("failed to marshal trace entry")
			http.Error(w, "failed to marshal trace entry", http.StatusInternalServerError)
			return
		}

		// Held across the write and the sync so the pair is atomic.
		writeMu.Lock()
		defer writeMu.Unlock()

		// write to file as a single line json entry
		_, err = traceFile.WriteString(string(traceData) + "\n")
		if err != nil {
			err = fmt.Errorf("failed to write trace data to file: %w", err)
			log.WithError(err).Error("trace stream failed")
			http.Error(w, "failed to write trace data to file", http.StatusInternalServerError)
			return
		}

		err = traceFile.Sync()
		if err != nil {
			err = fmt.Errorf("failed to sync trace file: %w", err)
			log.WithError(err).Error("trace stream failed")
			http.Error(w, "failed to sync trace file", http.StatusInternalServerError)
			return
		}

		w.WriteHeader(http.StatusCreated)
		w.Write([]byte("OK"))
	})

	return traceFile, nil
}

// Trace-stream failures are logged and answered with a 500, and deliberately
// NOT sent on the phone-home result channel.
//
// That channel carries the servicing outcome: netlisten's ListenLoop runs with
// waitForProvisioned=false, so any non-Failure result makes it return
// immediately via ToError. Enqueuing a telemetry error there would therefore
// abort the servicing run because its metrics could not be written, which is
// the wrong verdict -- the operation under test may have succeeded.
//
// Losing trace data is not silent either: the suite validates the captured
// trace file afterwards, so a missing metric is reported there, against the
// check that is actually about telemetry.
