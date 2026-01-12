package client

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"net"

	"tridenttools/pkg/rcp/tlscerts"

	"github.com/sirupsen/logrus"
)

// ListenAndAccept starts a TLS listener on the specified port and waits
// (blocking) for a single incoming connection. It returns the accepted
// connection and closes the listener.
//
// If the context is cancelled before a connection is accepted, it returns the
// context's error.
//
// The caller is responsible for closing the returned connection.
//
// This function uses mutual TLS authentication, requiring clients to present
// valid certificates. This listener is intended to be used by the RCP-proxy
// built with the same TLS setup. Any other clients or certificates will be
// rejected mutually.
func ListenAndAccept(ctx context.Context, certProvider tlscerts.CertProvider, port uint32) (net.Conn, error) {
	// Load our private server certificate
	cer, err := certProvider.LocalCert()
	if err != nil {
		return nil, fmt.Errorf("failed to load server certificate: %w", err)
	}

	// Create a certificate pool and load the client public certificate
	caCertPool := x509.NewCertPool()
	if !caCertPool.AppendCertsFromPEM(certProvider.RemoteCertPEM()) {
		return nil, fmt.Errorf("failed to load client CA certificate(s) into pool")
	}

	// Start a TLS listener
	listener, err := tls.Listen("tcp", fmt.Sprintf(":%d", port), &tls.Config{
		Certificates: []tls.Certificate{cer},
		ClientCAs:    caCertPool,
		ClientAuth:   tls.RequireAndVerifyClientCert,
		MinVersion:   tls.VersionTLS13,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to listen on port %d: %w", port, err)
	}
	defer listener.Close()

	logrus.Debugf("RCP-client listening on port %d", port)

	// Create a sub-context to handle listener closure on context cancellation.
	acceptCtx, cancel := context.WithCancel(ctx)
	defer cancel()

	go func() {
		// In the background, wait for context cancellation to close the
		// listener. This is necessary because Accept() is a blocking call so we
		// need to close the listener in parallel while the parent goroutine is
		// waiting. Closing the listener will cause Accept() to return an error
		// which we can handle appropriately.
		<-acceptCtx.Done()
		listener.Close()
	}()

	// Wait for an incoming connection
	conn, err := listener.Accept()
	if err != nil {
		if ctx.Err() != nil {
			// Context was cancelled
			return nil, ctx.Err()
		}

		// Some other error occurred
		return nil, fmt.Errorf("failed to accept connection: %w", err)
	}

	return conn, nil
}
