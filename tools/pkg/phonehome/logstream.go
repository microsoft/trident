package phonehome

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/fatih/color"
	log "github.com/sirupsen/logrus"
)

type LogEntry struct {
	Level   LogLevel `json:"level"`
	Message string   `json:"message"`
	Target  string   `json:"target"`
	Module  string   `json:"module"`
	File    string   `json:"file"`
	Line    int      `json:"line"`
}

type LogLevel string

const (
	LogLevelError LogLevel = "error"
	LogLevelWarn  LogLevel = "warn"
	LogLevelInfo  LogLevel = "info"
	LogLevelDebug LogLevel = "debug"
	LogLevelTrace LogLevel = "trace"
)

func SetupLogstream() {
	colorize := color.New(color.FgYellow).SprintfFunc()
	http.HandleFunc("/logstream", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(201)
		w.Write([]byte("OK"))

		var logEntry LogEntry
		err := json.NewDecoder(r.Body).Decode(&logEntry)
		if err != nil {
			log.WithError(err).Fatalf("failed to decode log entry")
			return
		}

		local_entry := log.WithFields(log.Fields{
			"module": logEntry.Module,
			"file":   logEntry.File,
			"line":   logEntry.Line,
		}).WithContext(r.Context())

		text := fmt.Sprintf("%s %s", colorize("[REMOTE %s]", logEntry.Target), logEntry.Message)

		switch logEntry.Level {
		case LogLevelError:
			local_entry.Error(text)
		case LogLevelWarn:
			local_entry.Warn(text)
		case LogLevelInfo:
			local_entry.Info(text)
		case LogLevelDebug:
			local_entry.Debug(text)
		case LogLevelTrace:
			local_entry.Trace(text)
		}
	})
}
