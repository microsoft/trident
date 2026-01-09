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
func StartReverseConnectProxy(ctx context.Context, clientAddress string, serverAddress string) error {
	logrus.Debugf("Starting reverse-connect proxy with client address: '%s' and server address: '%s'", clientAddress, serverAddress)
	cer, err := tlscerts.GetClientX509Cert()
	if err != nil {
		return fmt.Errorf("failed to load client certificate: %w", err)
	}

	caCertPool := x509.NewCertPool()
	caCertPool.AppendCertsFromPEM(tlscerts.GetServerCertPEM())

	tlsConfig := tls.Config{
		Certificates: []tls.Certificate{cer},
		RootCAs:      caCertPool,
		ServerName:   tlscerts.ServerSubjectAltName,
	}

	multipleRefused := false
	for {
		if ctx.Err() != nil {
			return ctx.Err()
		}

		// Run this in an anonymous function to allow defer closing connections
		func() {
			clientConn, err := establishClientConnection(clientAddress, &tlsConfig)
			if err != nil {
				var errno syscall.Errno
				if ok := errors.As(err, &errno); ok {
					if errno == syscall.ECONNREFUSED {
						if !multipleRefused {
							multipleRefused = true
							logrus.Warnf("Client connection refused, will retry silently.")
						}
						// Wait before retrying
						select {
						case <-time.After(1 * time.Second):
						case <-ctx.Done():
						}
					}
				} else {
					logrus.Errorf("Failed to establish client connection: %v", err)
				}

				return
			}
			defer clientConn.Close()

			err = runClientConnection(ctx, clientConn, serverAddress, &tlsConfig)
			if err != nil {
				logrus.Errorf("Client connection error[%T]: %v", err, err)
			}
		}()
	}
}

func establishClientConnection(clientAddress string, tlsConfig *tls.Config) (net.Conn, error) {
	clientConn, err := tls.Dial("tcp", clientAddress, tlsConfig)
	if err != nil {
		return nil, err
	}
	logrus.WithField("addr", clientConn.RemoteAddr()).Info("Connected to client")
	return clientConn, nil
}

func runClientConnection(ctx context.Context, clientConn net.Conn, serverAddress string, tlsConfig *tls.Config) error {
	logrus.Infof("Connecting to server at '%s'", serverAddress)
	serverConn, err := net.Dial("unix", serverAddress)
	if err != nil {
		return err
	}
	defer serverConn.Close()

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
