package proxies

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"path/filepath"
)

// ImageServer serves a single OS update image (e.g. a .cosi file) over plain
// HTTP so trident-aks-agent's fake Nebraska endpoint can point tridentd at a
// real, downloadable artifact during A/B update staging. tridentd downloads
// the image itself (not the aks-agent), so this just needs to serve the raw
// bytes at a stable path.
type ImageServer struct {
	// ImagePath is the local filesystem path to the image file to serve.
	ImagePath string
}

// Handler returns an http.Handler that serves ImagePath at the request path's
// base name (e.g. "/acl.cosi"), regardless of the requested path, so it works
// whether the caller mounts it at "/" or "/images/".
func (s *ImageServer) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		http.ServeFile(w, r, s.ImagePath)
	})
}

// ListenAndServe starts the image server on listenAddr and serves the image
// at every path under the given package name (so codebase + package name
// joins correctly regardless of trailing slash handling).
func (s *ImageServer) ListenAndServe(ctx context.Context, listenAddr string) (net.Listener, error) {
	listener, err := net.Listen("tcp", listenAddr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on %s: %w", listenAddr, err)
	}
	mux := http.NewServeMux()
	mux.Handle("/", s.Handler())
	server := &http.Server{Handler: mux}
	go func() {
		<-ctx.Done()
		_ = server.Shutdown(context.Background())
	}()
	go func() { _ = server.Serve(listener) }()
	return listener, nil
}

// PackageBaseName returns the file name portion of ImagePath, used as both
// the Nebraska package name and the served URL path segment.
func (s *ImageServer) PackageBaseName() string {
	return filepath.Base(s.ImagePath)
}
