package phonehome

import (
	"encoding/json"
	"net/http"
	"os"

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

func SetupTraceStream(filepath string) {
	// Setup a file to store the trace data
	var traceFile *os.File
	var err error
	if filepath != "" {
		traceFile, err = os.Create(filepath)
		if err != nil {
			log.WithError(err).Fatalf("failed to create trace file")
		}
	}

	// Generate a UUID to group events coming in from the same trident run
	traceID := uuid.New().String()

	http.HandleFunc("/tracestream", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(201)
		w.Write([]byte("OK"))

		var traceEntry TraceEntry
		err := json.NewDecoder(r.Body).Decode(&traceEntry)
		if err != nil {
			log.WithError(err).Fatalf("failed to decode trace entry")
			return
		}

		traceEntry.AdditionalFields["trace_id"] = traceID

		// write the trace data as json
		traceData, err := json.Marshal(traceEntry)
		if err != nil {
			log.WithError(err).Fatalf("failed to marshal trace entry")
			return
		}

		// if no file is provided, don't write the trace data to a file
		if traceFile == nil {
			return
		}

		// write to file as a single line json entry
		_, err = traceFile.WriteString(string(traceData) + "\n")
		if err != nil {
			log.WithError(err).Fatalf("failed to write trace data to file")
			return
		}
	})
}
