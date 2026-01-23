package main

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/signal"
	"syscall"
	"tridenttools/pkg/rcp/client"
	"tridenttools/pkg/rcp/tlscerts"

	"github.com/alecthomas/kong"
	"github.com/sirupsen/logrus"
)

const (
	defaultTridentBinaryLocation = "/usr/bin/trident"
	defaultOsmodifierLocation    = "/usr/bin/osmodifier"
	tridentInstallServiceName    = "trident-install.service"
)

var cli struct {
}

func main() {
	_ = kong.Parse(&cli,
		kong.UsageOnError(),
	)

	logrus.SetFormatter(&logrus.TextFormatter{
		ForceColors: true,
	})

	// Handle Ctrl+C gracefully
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	if err := run(ctx); err != nil {
		logrus.Fatalf("Error running test: %v", err)
	}
}

func run(ctx context.Context) error {
	rcpListener, err := client.ListenAndAccept(ctx, tlscerts.ServerCertProvider, 40079)
	if err != nil {
		return fmt.Errorf("failed to start RCP listener: %w", err)
	}

	logrus.Infof("Listening on: localhost:%d", rcpListener.Port)

	select {
	case <-ctx.Done():
		return nil
	case conn := <-rcpListener.ConnChan:
		logrus.Infof("Accepted RCP connection from %s", conn.RemoteAddr())
		return handleIncomingConnection(ctx, conn)
	}
}

func handleIncomingConnection(ctx context.Context, conn net.Conn) error {
	logrus.Infof("Handling incoming connection from %s", conn.RemoteAddr())
	// Placeholder for actual connection handling logic

	conn.Write([]byte("Hello from test RCP agent!\n"))

	<-ctx.Done()
	return nil
}
