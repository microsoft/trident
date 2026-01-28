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
	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v3"

	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/rcp"
	"tridenttools/pkg/rcp/proxy"
	"tridenttools/storm/utils/cmd"
)

const (
	defaultTridentBinaryLocation = "/usr/bin/trident"
	defaultOsmodifierLocation    = "/usr/bin/osmodifier"
	tridentInstallServiceName    = "trident-install.service"
)

var cli struct {
	Config string `short:"c" help:"Path to configuration file."`
}

func main() {
	_ = kong.Parse(
		&cli,
		kong.Description("A reverse-connect proxy that connects to an rcp-client to forward proxy connections between it and a server."),
		kong.UsageOnError(),
		kong.Vars{
			"defaultServerAddress": rcp.DefaultTridentSocketPath,
		},
	)
	logrus.SetFormatter(&logrus.TextFormatter{
		ForceColors: true,
	})

	// Set possible config file locations
	configFile := "/etc/rcp-agent/config.yaml"
	if cli.Config != "" {
		configFile = cli.Config
	}

	configData, err := os.ReadFile(configFile)
	if err != nil {
		logrus.Fatalf("Failed to read config file '%s': %v", configFile, err)
	}

	var config netlaunch.RcpAgentConfiguration
	err = yaml.Unmarshal(configData, &config)
	if err != nil {
		logrus.Fatalf("Failed to parse config file '%s': %v", configFile, err)
	}

	// Handle Ctrl+C gracefully
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	// Download files when provided in config
	for _, file := range config.AdditionalFiles {
		logrus.Infof("Downloading additional file from URL '%s' to destination '%s' with mode '%v'", file.DownloadUrl, file.Destination, file.Mode)
		if err := downloadFile(&file); err != nil {
			logrus.Fatalf("Failed to download additional file: %v", err)
		}
	}

	if config.ClientAddress == "" {
		logrus.Warn("No client address specified, running legacy Trident install service.")

		err := enableAndStartTridentInstallService()
		if err != nil {
			logrus.Fatalf("Failed to enable and start Trident install service: %v", err)
		}

		return
	}

	logrus.Infof("Starting reverse-connect proxy with client address: '%s' and server address: '%s'", config.ClientAddress, config.ServerAddress)
	if err := proxy.StartReverseConnectProxy(ctx, &config.RcpClientTls, config.ClientAddress, config.ServerAddress, time.Second); err != nil {
		logrus.Fatalf("reverse-connect proxy error: %v", err)
	}
	logrus.Info("Shutdown complete")
}

func downloadFile(file *netlaunch.RcpAdditionalFile) error {
	parent := filepath.Dir(file.Destination)
	if err := os.MkdirAll(parent, 0755); err != nil {
		return fmt.Errorf("failed to create parent directory '%s': %w", parent, err)
	}

	out, err := os.OpenFile(file.Destination, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, file.Mode)
	if err != nil {
		return fmt.Errorf("failed to create file '%s': %w", file.Destination, err)
	}
	defer out.Close()

	resp, err := http.Get(file.DownloadUrl)
	if err != nil {
		return fmt.Errorf("failed to download file from URL '%s': %w", file.DownloadUrl, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("failed to download file from URL '%s': received status code %d", file.DownloadUrl, resp.StatusCode)
	}

	_, err = io.Copy(out, resp.Body)
	if err != nil {
		return fmt.Errorf("failed to write to file '%s': %w", file.Destination, err)
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
