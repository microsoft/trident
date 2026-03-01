package netlaunch

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"syscall"
	"time"
	rcpclient "tridenttools/pkg/rcp/client"

	"github.com/sirupsen/logrus"
	log "github.com/sirupsen/logrus"
)

// openLocalProxy sets up a local proxy that listens on the specified socket
// path and forwards connections to the netlaunch server. Only one connection
// will be accepted and forwarded at a time. When a connection closes, the proxy
// will accept a new connection.
func openLocalProxy(ctx context.Context, socketPath string, rcpListener *rcpclient.RcpListener) error {
	for {
		log.Info("Waiting for RCP connection...")
		select {
		case <-ctx.Done():
			return ctx.Err()
		case conn, ok := <-rcpListener.ConnChan:
			if !ok || conn == nil {
				return fmt.Errorf("RCP listener closed")
			}
			log.Infof("Accepted RCP connection from %s", conn.RemoteAddr())
			err := runLocalProxy(ctx, socketPath, conn)
			if err != nil {
				log.Errorf("Error running local proxy: %v", err)
			}

			if ctx.Err() != nil {
				// Don't retry if the context was cancelled.
				return ctx.Err()
			}
		}
	}
}

func runLocalProxy(ctx context.Context, socketPath string, remoteConn net.Conn) error {
	defer remoteConn.Close()
	// Remove any existing socket file to avoid "address already in use" errors.
	if err := os.Remove(socketPath); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("failed to remove existing socket %s: %w", socketPath, err)
	}

	listener, err := net.Listen("unix", socketPath)
	if err != nil {
		return fmt.Errorf("failed to listen on unix socket %s: %w", socketPath, err)
	}
	defer listener.Close()
	defer os.Remove(socketPath)

	log.WithField("socket", socketPath).Info("Local proxy listening")

	// Close the listener when the context is cancelled so Accept unblocks.
	go func() {
		<-ctx.Done()
		listener.Close()
	}()

	localConn, err := listener.Accept()
	if err != nil {
		// Check if the context was cancelled.
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return fmt.Errorf("failed to accept connection: %w", err)
	}

	log.WithField("addr", localConn.RemoteAddr()).Info("Accepted local proxy connection")

	// Forward data bidirectionally between the local and remote connections.
	// Block until the forwarding completes (one connection at a time).
	forwardConnections(ctx, localConn, remoteConn)

	return nil
}

// forwardConnections copies data bidirectionally between two connections.
// It blocks until both directions are finished (i.e. one side closes or errors).
func forwardConnections(ctx context.Context, local, remote net.Conn) {
	defer local.Close()

	local.SetReadDeadline(time.Time{})
	remote.SetReadDeadline(time.Time{})

	// Channel to signal when copying is done. Buffered to allow both goroutines
	// to send without blocking.
	doneChan := make(chan string, 2)

	// Start the proxying
	go func() {
		_, err := io.Copy(remote, local)
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
		doneChan <- "local->remote"
	}()
	go func() {
		_, err := io.Copy(local, remote)
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
		doneChan <- "remote->local"
	}()

	// Wait for either copy to finish or context cancellation
	select {
	case direction := <-doneChan:
		logrus.Infof("Connection closed by '%s'", direction)
	case <-ctx.Done():
		logrus.Info("Context cancelled")
	}
}
