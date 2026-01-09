//go:build tls_client

package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/alecthomas/kong"
	"github.com/sirupsen/logrus"

	"tridenttools/pkg/rcp"
	"tridenttools/pkg/rcp/proxy"
)

var cli struct {
	ClientAddress string `arg:"" help:"Address of the superproxy to connect to"`
	ServerAddress string `short:"s" long:"server-address" help:"Address of the server server to connect to" default:"${defaultServerAddress}"`
}

func main() {
	_ = kong.Parse(&cli,
		kong.Name("rcp-proxy"),
		kong.Description("A reverse-connect proxy that connects to an rcp-client to forward proxy connections between it and a server."),
		kong.UsageOnError(),
		kong.Vars{
			"defaultServerAddress": rcp.DefaultTridentSocketPath,
		},
	)

	// Handle Ctrl+C gracefully
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, os.Interrupt, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		sig := <-sigChan
		logrus.Warnf("Received signal %v, shutting down gracefully...", sig)
		cancel()
	}()

	logrus.Infof("Starting reverse-connect proxy with client address: '%s' and server address: '%s'", cli.ClientAddress, cli.ServerAddress)
	if err := proxy.StartReverseConnectProxy(ctx, cli.ClientAddress, cli.ServerAddress, time.Second); err != nil {
		logrus.Fatalf("Superproxy error: %v", err)
	}
	logrus.Info("Shutdown complete")
}
