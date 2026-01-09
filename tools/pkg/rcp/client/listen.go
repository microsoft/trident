//go:build tls_server

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

type RcpConn struct {
	listener net.Listener
}

func Listen(ctx context.Context, port uint32) (net.Conn, error) {
	var err error

	cer, err := tlscerts.GetServerX509Cert()
	if err != nil {
		return nil, fmt.Errorf("failed to load server certificate: %w", err)
	}

	caCertPool := x509.NewCertPool()
	caCertPool.AppendCertsFromPEM(tlscerts.GetClientCertPEM())

	listener, err := tls.Listen("tcp", fmt.Sprintf(":%d", port), &tls.Config{
		Certificates: []tls.Certificate{cer},
		ClientCAs:    caCertPool,
		ClientAuth:   tls.RequireAndVerifyClientCert,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to listen on port %d: %w", port, err)
	}
	defer listener.Close()

	logrus.Debugf("RCP-client listening on port %d", port)

	go func() {
		<-ctx.Done()
		logrus.Debug("Shutting down RCP-client listener")
		listener.Close()
	}()

	conn, err := listener.Accept()
	if err != nil {
		if ctx.Err() != nil {
			logrus.Debug("Listener closed due to context cancellation")
			return nil, nil
		}

		return nil, fmt.Errorf("failed to accept connection: %w", err)
	}

	logrus.Debug("RCP-client accepted connection")
	return conn, nil
}
