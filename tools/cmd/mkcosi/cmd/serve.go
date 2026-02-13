package cmd

import (
	"fmt"
	"net"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/dustin/go-humanize"
	log "github.com/sirupsen/logrus"
)

type ServeCmd struct {
	Directory string `arg:"" help:"Directory to serve files from." type:"existingdir"`
	Port      int    `short:"p" help:"Port to listen on." default:"0"`
	Verbose   bool   `short:"v" help:"Enable verbose logging."`
}

func (r *ServeCmd) Run() error {
	if r.Verbose {
		log.SetLevel(log.DebugLevel)
	}

	// Create file server
	fileServer := http.FileServer(http.Dir(r.Directory))

	// Wrap with logging handler
	handler := &loggingHandler{handler: fileServer}

	addr := fmt.Sprintf(":%d", r.Port)

	// Create listener first so we can get the actual port when using port 0
	listener, err := net.Listen("tcp", addr)
	if err != nil {
		log.WithError(err).Error("Failed to create listener")
		return err
	}

	// Extract the actual port from the listener
	actualPort := listener.Addr().(*net.TCPAddr).Port
	log.WithFields(log.Fields{
		"directory": r.Directory,
		"port":      actualPort,
	}).Info("Starting file server")

	if err := http.Serve(listener, handler); err != nil {
		log.WithError(err).Error("Server failed")
		return err
	}

	return nil
}

// loggingHandler wraps an http.Handler and logs each request
type loggingHandler struct {
	handler http.Handler
}

func (h *loggingHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	start := time.Now()

	// Wrap the ResponseWriter to capture the status code
	lrw := &loggingResponseWriter{ResponseWriter: w, statusCode: http.StatusOK}

	// Serve the request
	h.handler.ServeHTTP(lrw, r)

	// Log request details
	fields := log.Fields{
		"method":   r.Method,
		"path":     r.URL.Path,
		"client":   r.RemoteAddr,
		"status":   lrw.statusCode,
		"duration": time.Since(start).String(),
	}

	// Check for range request
	if rangeHeader := r.Header.Get("Range"); rangeHeader != "" {
		fields["range"] = rangeHeader
		// Parse range to show human-readable size
		if size := parseRangeSize(rangeHeader); size > 0 {
			fields["range_size"] = humanize.Bytes(uint64(size))
		}
	}

	log.WithFields(fields).Info("Request served")
}

// loggingResponseWriter captures the status code
type loggingResponseWriter struct {
	http.ResponseWriter
	statusCode int
}

func (lrw *loggingResponseWriter) WriteHeader(code int) {
	lrw.statusCode = code
	lrw.ResponseWriter.WriteHeader(code)
}

// parseRangeSize parses a Range header and returns the size of the range in bytes.
// Returns 0 if the range cannot be parsed or is open-ended.
func parseRangeSize(rangeHeader string) int64 {
	// Range header format: "bytes=start-end" or "bytes=start-"
	if !strings.HasPrefix(rangeHeader, "bytes=") {
		return 0
	}

	rangeSpec := strings.TrimPrefix(rangeHeader, "bytes=")
	parts := strings.Split(rangeSpec, "-")
	if len(parts) != 2 {
		return 0
	}

	start, err := strconv.ParseInt(parts[0], 10, 64)
	if err != nil {
		return 0
	}

	// Open-ended range (e.g., "bytes=100-")
	if parts[1] == "" {
		return 0
	}

	end, err := strconv.ParseInt(parts[1], 10, 64)
	if err != nil {
		return 0
	}

	// Range is inclusive, so size is end - start + 1
	return end - start + 1
}
