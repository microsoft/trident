//go:build tls_client

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
func StartReverseConnectProxy(ctx context.Context, clientAddress string, serverAddress string, retryInterval time.Duration) error {
	// Load our private client certificate
	cer, err := tlscerts.GetClientX509Cert()
	if err != nil {
		return fmt.Errorf("failed to load client certificate: %w", err)
	}

	// Create a certificate pool and load the server public certificate
	caCertPool := x509.NewCertPool()
	caCertPool.AppendCertsFromPEM(tlscerts.GetServerCertPEM())

	// Configure TLS with mutual authentication
	tlsConfig := tls.Config{
		Certificates: []tls.Certificate{cer},
		RootCAs:      caCertPool,
		ServerName:   tlscerts.ServerSubjectAltName,
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
			if ok := errors.As(err, &errno); ok {
				if errno == syscall.ECONNREFUSED {
					if !multipleRefused {
						multipleRefused = true
						logrus.Warnf("Client connection refused, will retry silently.")
					}

					// Wait before retrying
					select {
					case <-time.After(retryInterval):
					case <-ctx.Done():
						// Context cancelled, exit loop
						return ctx.Err()
					}
				}
			} else {
				// Some other error occurred
				logrus.Errorf("Failed to establish client connection: %v", err)
			}

			continue
		}

		// Successful connection, reset refused flag
		multipleRefused = false

		// Handle the client connection, the function will block until the
		// connection is closed or an error occurs and close the connection.
		logrus.Infof("Client connected from '%s'", clientConn.RemoteAddr().String())
		err = handleClientConnection(ctx, clientConn, serverAddress)
		if err != nil {
			logrus.Errorf("Client connection error: %v", err)
		}
	}
}

// handleClientConnection handles a single client connection by connecting to
// the server and proxying data between the client and server connections.
//
// This function blocks until the connection is closed or an error occurs.
func handleClientConnection(ctx context.Context, clientConn net.Conn, serverAddress string) error {
	defer clientConn.Close()
	logrus.Infof("Client connected from '%s'", clientConn.RemoteAddr().String())

	logrus.Infof("Connecting to server at '%s'", serverAddress)
	serverConn, err := net.Dial("unix", serverAddress)
	if err != nil {
		return fmt.Errorf("failed to connect to server at '%s': %w", serverAddress, err)
	}
	defer serverConn.Close()

	// Proxy data between client and server
	return proxyConnections(ctx, clientConn, serverConn)
}

// proxyConnections proxies data between the clientConn and serverConn until
// either connection is closed or the context is cancelled.
//
// This function blocks until the connection is closed or an error occurs.
func proxyConnections(ctx context.Context, clientConn net.Conn, serverConn net.Conn) error {
	doneChan := make(chan string)

	// Start the proxying
	go func() {
		_, err := io.Copy(serverConn, clientConn)
		if err != nil {
			logrus.Errorf("Error copying from client to server: %v", err)
		}
		doneChan <- "client->server"
	}()
	go func() {
		_, err := io.Copy(clientConn, serverConn)
		if err != nil {
			logrus.Errorf("Error copying from server to client: %v", err)
		}
		doneChan <- "server->client"
	}()

	// Wait for either copy to finish or context cancellation
	select {
	case closer := <-doneChan:
		logrus.Infof("Connection closed by '%s'", closer)
	case <-ctx.Done():
		logrus.Info("Context cancelled")
	}

	return nil
}
