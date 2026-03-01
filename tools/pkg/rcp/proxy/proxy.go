package proxy

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net"
	"syscall"
	"time"

	"github.com/sirupsen/logrus"

	"tridenttools/pkg/rcp/tlscerts"
)

// StartReverseConnectProxy starts a Reverse Connect Proxy that connects to a
// client at clientAddress and once a TLS connection is established, connects to
// a server at serverAddress, then proxies data between the two connections.
//
// This is a blocking call that runs until the context is cancelled.
//
// Both client and server connections use mutual TLS authentication, requiring
// valid certificates. This proxy is intended to be used by the RCP-client built
// with the same TLS setup. Any other certificates will be rejected mutually.
func StartReverseConnectProxy(
	ctx context.Context,
	certProvider tlscerts.CertProvider,
	clientAddress string,
	serverAddress string,
	serverConnectionType string,
	retryInterval time.Duration,
) error {
	// Load our private client certificate
	cer, err := certProvider.LocalCert()
	if err != nil {
		return fmt.Errorf("failed to load client certificate: %w", err)
	}

	// Create a certificate pool and load the server public certificate
	caCertPool := x509.NewCertPool()
	if ok := caCertPool.AppendCertsFromPEM(certProvider.RemoteCertPEM()); !ok {
		return fmt.Errorf("failed to load server CA certificate(s) into pool")
	}

	// Configure TLS with mutual authentication
	tlsConfig := tls.Config{
		Certificates: []tls.Certificate{cer},
		RootCAs:      caCertPool,
		ServerName:   tlscerts.ServerSubjectAltName,
		MinVersion:   tls.VersionTLS13,
	}

	// Bool to keep track if the connection has been refused multiple times
	// consecutively, to reduce log spam.
	multipleRefused := false

	// Main loop to keep trying to connect to the client
	for {
		if ctx.Err() != nil {
			// Context cancelled, exit loop
			return ctx.Err()
		}

		// Try to establish connection to the client
		clientConn, err := tls.Dial("tcp", clientAddress, &tlsConfig)
		if err != nil {
			var errno syscall.Errno

			// Check if this is a connection refused error
			if ok := errors.As(err, &errno); ok && errno == syscall.ECONNREFUSED {
				if !multipleRefused {
					multipleRefused = true
					logrus.Warnf("Client connection refused, will retry silently.")
				}
			} else {
				// Some other error occurred
				logrus.Errorf("Failed to establish client connection: %v", err)
			}

			// Wait before retrying
			select {
			case <-time.After(retryInterval):
			case <-ctx.Done():
				// Context cancelled, exit loop
				return ctx.Err()
			}

			continue
		}

		// Successful connection, reset refused flag
		multipleRefused = false

		// Handle the client connection, the function will block until the
		// connection is closed or an error occurs and close the connection.
		logrus.Infof("Client connected from '%s'", clientConn.RemoteAddr().String())
		err = handleClientConnection(ctx, clientConn, serverAddress, serverConnectionType)
		if err != nil {
			logrus.Errorf("Client connection error: %v", err)
		}
	}
}

// handleClientConnection handles a single client connection by connecting to
// the server and proxying data between the client and server connections.
//
// This function blocks until the connection is closed or an error occurs.
func handleClientConnection(
	ctx context.Context,
	clientConn net.Conn,
	serverAddress string,
	serverConnectionType string,
) error {
	defer clientConn.Close()

	logrus.Infof("Connecting to server at '%s'", serverAddress)
	serverConn, err := net.Dial(serverConnectionType, serverAddress)
	if err != nil {
		return fmt.Errorf("failed to connect to server at '%s': %w", serverAddress, err)
	}
	defer serverConn.Close()

	// Both connections are established, start proxying data between them

	// Channel to signal when copying is done. Buffered to allow both goroutines
	// to send without blocking.
	doneChan := make(chan string, 2)

	// Start the proxying
	go func() {
		_, err := io.Copy(serverConn, clientConn)
		if err != nil {
			switch {
			case errors.Is(err, io.EOF),
				errors.Is(err, net.ErrClosed),
				errors.Is(err, syscall.EPIPE):
				logrus.Debugf("Connection closed while copying from client to server: %v", err)
			default:
				logrus.Errorf("Error copying from client to server: %v", err)
			}
		}
		doneChan <- "client->server"
	}()
	go func() {
		_, err := io.Copy(clientConn, serverConn)
		if err != nil {
			switch {
			case errors.Is(err, io.EOF),
				errors.Is(err, net.ErrClosed),
				errors.Is(err, syscall.EPIPE):
				logrus.Debugf("Connection closed while copying from server to client: %v", err)
			default:
				logrus.Errorf("Error copying from server to client: %v", err)
			}
		}
		doneChan <- "server->client"
	}()

	// Wait for either copy to finish or context cancellation
	select {
	case direction := <-doneChan:
		logrus.Infof("Connection closed by '%s'", direction)
	case <-ctx.Done():
		logrus.Info("Context cancelled")
	}

	return nil
}
