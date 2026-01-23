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

// RcpListener encapsulates a channel that will receive an incoming connection
// and the port number the listener is bound to.
type RcpListener struct {
	ConnChan <-chan net.Conn
	Port     uint16
	cancel   context.CancelFunc
}

func (l *RcpListener) Close() {
	if l.cancel != nil {
		l.cancel()
	}
}

// ListenAndAccept starts a TLS listener on the specified port and returns an
// RcpListener containing a channel that will receive an incoming connection and
// the port number the listener is bound to.
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
func ListenAndAccept(ctx context.Context, certProvider tlscerts.CertProvider, port uint16) (*RcpListener, error) {
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

	// If port 0 was specified, get the actual assigned port.
	port = uint16(listener.Addr().(*net.TCPAddr).Port)

	logrus.Debugf("RCP-client listening on port %d", port)

	// Create a sub-context to handle listener closure on context cancellation.
	acceptCtx, acceptCancel := context.WithCancel(ctx)

	go func() {
		// In the background, wait for context cancellation to close the
		// listener. This is necessary because Accept() is a blocking call so we
		// need to close the listener in parallel while the parent goroutine is
		// waiting. Closing the listener will cause Accept() to return an error
		// which we can handle appropriately.
		<-acceptCtx.Done()
		logrus.Debug("RCP-client listener context cancelled, closing listener")
		listener.Close()
	}()

	connChan := make(chan net.Conn, 1)

	// Wait for an incoming connection
	go func() {
		defer close(connChan)
		for {
			conn, err := listener.Accept()
			if err == nil {
				logrus.Infof("RCP accepted connection from %s", conn.RemoteAddr().String())
				acceptCancel() // Stop the listener-closure goroutine
				connChan <- conn
				return
			}

			logrus.Errorf("Failed to accept connection: %v", err)
		}
	}()

	return &RcpListener{
		ConnChan: connChan,
		Port:     port,
		cancel:   acceptCancel,
	}, nil
}
