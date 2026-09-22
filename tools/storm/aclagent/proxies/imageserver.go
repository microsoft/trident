package proxies

import (
	"context"
	"crypto/tls"
	"fmt"
	"net"
	"net/http"
	"path/filepath"
)

// ImageServer serves a single OS update image (e.g. a .cosi file) over HTTPS
// so trident-acl-agent's fake Nebraska endpoint can point tridentd at a
// real, downloadable artifact during A/B update staging. tridentd downloads
// the image itself (not the acl-agent), so this just needs to serve the raw
// bytes at a stable path.
type ImageServer struct {
	// ImagePath is the local filesystem path to the image file to serve.
	ImagePath string

	// Cert is the TLS certificate ListenAndServe presents to clients (e.g.
	// from GenerateEphemeralTLSCert). Callers are expected to have arranged
	// for its PEM to be trusted by the VM's system trust store before
	// tridentd downloads the image - trident-acl-agent/tridentd never skip
	// or downgrade TLS verification for this server.
	Cert tls.Certificate
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
// joins correctly regardless of trailing slash handling). It serves over
// TLS using s.Cert.
func (s *ImageServer) ListenAndServe(ctx context.Context, listenAddr string) (net.Listener, error) {
	listener, err := net.Listen("tcp", listenAddr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on %s: %w", listenAddr, err)
	}
	mux := http.NewServeMux()
	mux.Handle("/", s.Handler())
	server := &http.Server{
		Handler: mux,
		// See the matching comment in NebraskaProxy.ListenAndServe: capping
		// at TLS 1.2 avoids a "tls: bad record MAC" interop bug between
		// Go's TLS 1.3 server and the client TLS stack tridentd/
		// trident-acl-agent link against.
		TLSConfig: &tls.Config{Certificates: []tls.Certificate{s.Cert}, MaxVersion: tls.VersionTLS12},
	}
	go func() {
		<-ctx.Done()
		_ = server.Shutdown(context.Background())
	}()
	// certFile/keyFile are empty: TLSConfig.Certificates is already
	// populated above, which ServeTLS uses directly.
	go func() { _ = server.ServeTLS(listener, "", "") }()
	return listener, nil
}

// PackageBaseName returns the file name portion of ImagePath, used as both
// the Nebraska package name and the served URL path segment.
func (s *ImageServer) PackageBaseName() string {
	return filepath.Base(s.ImagePath)
}
