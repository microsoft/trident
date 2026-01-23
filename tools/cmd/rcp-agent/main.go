//go:build tls_client

package main

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/alecthomas/kong"
	kongyaml "github.com/alecthomas/kong-yaml"
	"github.com/sirupsen/logrus"

	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/rcp"
	"tridenttools/pkg/rcp/proxy"
	"tridenttools/pkg/rcp/tlscerts"
	"tridenttools/storm/utils/cmd"
)

const (
	defaultTridentBinaryLocation = "/usr/bin/trident"
	defaultOsmodifierLocation    = "/usr/bin/osmodifier"
	tridentInstallServiceName    = "trident-install.service"
)

var cli netlaunch.RcpAgentConfiguration

func main() {
	_ = kong.Parse(&cli,
		kong.Description("A reverse-connect proxy that connects to an rcp-client to forward proxy connections between it and a server."),
		kong.UsageOnError(),
		kong.Vars{
			"defaultServerAddress": rcp.DefaultTridentSocketPath,
		},
		kong.Configuration(kongyaml.Loader, "/etc/rcp-agent/config.yaml", "./rcp-agent.yaml"),
	)

	logrus.SetFormatter(&logrus.TextFormatter{
		ForceColors: true,
	})

	// Handle Ctrl+C gracefully
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	// Download Trident when a URL is provided
	if cli.TridentDownloadUrl != "" {
		logrus.Infof("Trident download URL provided, downloading Trident.")
		if err := downloadTrident(cli.TridentDownloadUrl); err != nil {
			logrus.Fatalf("Failed to download Trident: %v", err)
		}
	}

	if cli.OsmodifierDownloadUrl != "" {
		logrus.Infof("Osmodifier download URL provided, downloading Osmodifier.")
		if err := downloadOsmodifier(cli.OsmodifierDownloadUrl); err != nil {
			logrus.Fatalf("Failed to download Osmodifier: %v", err)
		}
	}

	if cli.ClientAddress == "" {
		logrus.Warn("No client address specified, running legacy Trident install service.")

		err := enableAndStartTridentInstallService()
		if err != nil {
			logrus.Fatalf("Failed to enable and start Trident install service: %v", err)
		}

		return
	}

	logrus.Infof("Starting reverse-connect proxy with client address: '%s' and server address: '%s'", cli.ClientAddress, cli.ServerAddress)
	if err := proxy.StartReverseConnectProxy(ctx, tlscerts.ClientCertProvider, cli.ClientAddress, cli.ServerAddress, time.Second); err != nil {
		logrus.Fatalf("reverse-connect proxy error: %v", err)
	}
	logrus.Info("Shutdown complete")
}

func downloadTrident(url string) error {
	logrus.Infof("Downloading Trident from URL: %s", url)
	err := downloadExecutableFile(url, defaultTridentBinaryLocation)
	if err != nil {
		return fmt.Errorf("failed to download Trident: %w", err)
	}

	return nil
}

func downloadOsmodifier(url string) error {
	logrus.Infof("Downloading Osmodifier from URL: %s", url)
	err := downloadExecutableFile(url, defaultOsmodifierLocation)
	if err != nil {
		return fmt.Errorf("failed to download Osmodifier: %w", err)
	}

	return nil
}

func downloadExecutableFile(url string, destinationPath string) error {
	parent := filepath.Dir(destinationPath)
	if err := os.MkdirAll(parent, 0755); err != nil {
		return fmt.Errorf("failed to create parent directory '%s': %w", parent, err)
	}

	out, err := os.Create(destinationPath)
	if err != nil {
		return fmt.Errorf("failed to create file '%s': %w", destinationPath, err)
	}
	defer out.Close()

	resp, err := http.Get(url)
	if err != nil {
		return fmt.Errorf("failed to download file from URL '%s': %w", url, err)
	}
	defer resp.Body.Close()

	_, err = io.Copy(out, resp.Body)
	if err != nil {
		return fmt.Errorf("failed to write to file '%s': %w", destinationPath, err)
	}

	// Now make the binary executable
	err = os.Chmod(destinationPath, 0755)
	if err != nil {
		return fmt.Errorf("failed to make binary executable: %w", err)
	}

	return nil
}

func enableAndStartTridentInstallService() error {
	logrus.Infof("Enabling and starting %s", tridentInstallServiceName)

	err := cmd.Run("systemctl", "enable", tridentInstallServiceName)
	if err != nil {
		return fmt.Errorf("failed to enable %s: %w", tridentInstallServiceName, err)
	}

	err = cmd.Run("systemctl", "start", "--no-block", tridentInstallServiceName)
	if err != nil {
		return fmt.Errorf("failed to start %s: %w", tridentInstallServiceName, err)
	}

	return nil
}
