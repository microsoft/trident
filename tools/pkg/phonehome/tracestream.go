package phonehome

import (
	"encoding/json"
	"fmt"
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

func SetupTraceStream(mux *http.ServeMux, filepath string, result chan<- PhoneHomeResult) (*os.File, error) {
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

		// write to file as a single line json entry
		_, err = traceFile.WriteString(string(traceData) + "\n")
		if err != nil {
			err = fmt.Errorf("failed to write trace data to file: %w", err)
			log.WithError(err).Error("trace stream failed")
			reportTraceStreamError(result, err)
			http.Error(w, "failed to write trace data to file", http.StatusInternalServerError)
			return
		}

		err = traceFile.Sync()
		if err != nil {
			err = fmt.Errorf("failed to sync trace file: %w", err)
			log.WithError(err).Error("trace stream failed")
			reportTraceStreamError(result, err)
			http.Error(w, "failed to sync trace file", http.StatusInternalServerError)
			return
		}

		w.WriteHeader(http.StatusCreated)
		w.Write([]byte("OK"))
	})

	return traceFile, nil
}

func reportTraceStreamError(result chan<- PhoneHomeResult, err error) {
	if result == nil {
		return
	}

	select {
	case result <- errorPhoneHomeResult(err):
	default:
		log.WithError(err).Error("could not report trace stream error")
	}
}
