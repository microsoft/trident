package phonehome

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"

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

func (l LogLevel) AsLogrusLevel() log.Level {
	switch l {
	case LogLevelError:
		return log.ErrorLevel
	case LogLevelWarn:
		return log.WarnLevel
	case LogLevelInfo:
		return log.InfoLevel
	case LogLevelDebug:
		return log.DebugLevel
	case LogLevelTrace:
		return log.TraceLevel
	default:
		log.WithField("level", l).Warn("Received unknown log level, defaulting to info")
		return log.InfoLevel
	}
}

func SetupLogstream(backgroundLogFile string) (*os.File, error) {
	colorize := color.New(color.FgYellow).SprintfFunc()

	// Setup background logger
	bgLogger := log.New()
	bgLogger.SetLevel(log.TraceLevel)
	bgLogFile, err := os.Create(backgroundLogFile)
	if err != nil {
		log.WithError(err).WithField("file", backgroundLogFile).Fatalf("failed to create background log file")
		return nil, err
	}
	bgLogger.SetOutput(bgLogFile)
	bgLogger.SetFormatter(&log.TextFormatter{
		ForceColors: true,
	})

	http.HandleFunc("/logstream", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(201)
		w.Write([]byte("OK"))

		var logEntry LogEntry
		err := json.NewDecoder(r.Body).Decode(&logEntry)
		if err != nil {
			log.WithError(err).Fatalf("failed to decode log entry")
			return
		}

		logLevel := logEntry.Level.AsLogrusLevel()

		// Fields we populate in all cases
		populate_fields := func(l *log.Logger) *log.Entry {
			return l.WithFields(log.Fields{
				"module": logEntry.Module,
				"file":   logEntry.File,
				"line":   logEntry.Line,
			})
		}

		// Always log to background log file
		populate_fields(bgLogger).Log(
			logLevel,
			fmt.Sprintf("[%s] %s", logEntry.Target, logEntry.Message),
		)

		// Only log to stdout if log level is debug or more important
		if logLevel <= log.DebugLevel {
			text := fmt.Sprintf("%s %s", colorize("[REMOTE %s]", logEntry.Target), logEntry.Message)
			populate_fields(log.StandardLogger()).Log(logLevel, text)
		}
	})

	return bgLogFile, nil
}
