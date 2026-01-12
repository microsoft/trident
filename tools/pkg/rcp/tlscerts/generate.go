//go:build ignore

// This is a standalone tool to generate self-signed certificates for testing
// purposes meant to be run by go generate.

package main

import (
	"fmt"
	"os"
	"os/exec"

	"github.com/alecthomas/kong"
	"github.com/sirupsen/logrus"
)

// outputFiles is the list of files to be generated.
var outputFiles = []string{
	"server.crt",
	"server.key",
	"client.crt",
	"client.key",
}

type cli struct {
	Generate generateCmd `cmd:"" help:"Generate self-signed TLS certs (no-op if already present)."`
	Clean    cleanCmd    `cmd:"" help:"Remove generated TLS cert files."`
}

type generateCmd struct {
	ServerSubjectAltName string `name:"san" help:"Subject Alternative Name for the server certificate." default:"reverseconnectproxy"`
}

func (c *generateCmd) Run() error {
	return generateCerts("DNS:" + c.ServerSubjectAltName)
}

type cleanCmd struct{}

func (cleanCmd) Run() error {
	return cleanCertFiles()
}

func main() {
	logrus.SetLevel(logrus.DebugLevel)
	logrus.SetFormatter(&logrus.TextFormatter{
		ForceColors: true,
	})

	var app cli
	ctx := kong.Parse(
		&app,
		kong.Name("tlscerts"),
		kong.Description("Generate or clean self-signed TLS certs for testing."),
	)
	if err := ctx.Run(); err != nil {
		logrus.WithError(err).Fatal("command failed")
	}
}

func generateCerts(subjectAltName string) error {
	if checkAllFilesExist() {
		logrus.Info("Skipping certificate generation")
		return nil
	}

	// Openssl command arguments to generate the certs
	cmdArgs := [][]string{
		{"genrsa", "-out", "server.key", "2048"},
		{"req", "-new", "-x509", "-sha256", "-key", "server.key", "-out", "server.crt", "-days", "365", "-subj", "/CN=localhost", "-addext", "subjectAltName=" + subjectAltName},
		{"genrsa", "-out", "client.key", "2048"},
		{"req", "-new", "-x509", "-sha256", "-key", "client.key", "-out", "client.crt", "-days", "365", "-subj", "/CN=localhost"},
	}

	for _, args := range cmdArgs {
		logrus.Debugf("Running command: %v", args)
		err := exec.Command("openssl", args[:]...).Run()
		if err != nil {
			return fmt.Errorf("failed to run openssl command with args %v: %w", args, err)
		}
	}
	return nil
}

func checkAllFilesExist() bool {
	for _, file := range outputFiles {
		stat, err := os.Stat(file)
		if err != nil {
			if os.IsNotExist(err) {
				logrus.Infof("File '%s' does not exist", file)
				return false
			}

			logrus.WithError(err).Warnf("Failed to stat %q", file)
			return false
		}

		if stat.IsDir() {
			logrus.Infof("File '%s' is a directory", file)
			return false
		}
	}

	logrus.Info("All files exist")
	return true
}

func cleanCertFiles() error {
	for _, file := range outputFiles {
		stat, err := os.Stat(file)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}

			return fmt.Errorf("failed to stat %q: %w", file, err)
		}

		if stat.IsDir() {
			return fmt.Errorf("refusing to remove directory %q", file)
		}

		if err := os.Remove(file); err != nil {
			if os.IsNotExist(err) {
				continue
			}

			return fmt.Errorf("failed to remove %q: %w", file, err)
		}

		logrus.Infof("Removed %q", file)
	}
	return nil
}
