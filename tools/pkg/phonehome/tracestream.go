package phonehome

import (
	"encoding/json"
	"net/http"
	"os"

	log "github.com/sirupsen/logrus"
)

type TraceEntry struct {
	Timestamp  string      `json:"timestamp"`
	AssetId    string      `json:"asset_id"`
	MetricName string      `json:"metric_name"`
	Value      interface{} `json:"value"`
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
	http.HandleFunc("/tracestream", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(201)
		w.Write([]byte("OK"))

		var traceEntry TraceEntry
		err := json.NewDecoder(r.Body).Decode(&traceEntry)
		if err != nil {
			log.WithError(err).Fatalf("failed to decode trace entry")
			return
		}

		// write the trace data as json
		traceData, err := json.Marshal(traceEntry)
		if err != nil {
			log.WithError(err).Fatalf("failed to marshal trace entry")
			return
		}

		log.WithFields(
			log.Fields{
				"timestamp":   traceEntry.Timestamp,
				"asset_id":    traceEntry.AssetId,
				"metric_name": traceEntry.MetricName,
				"value":       traceEntry.Value,
			},
		).Debug("Recieved a tracing event")

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
		log.Debug("Wrote trace event to file")
	})
}
