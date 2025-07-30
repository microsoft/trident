// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"azltools/imagegen/attendedinstaller"
	"azltools/internal/logger"
	"fmt"
	"os"
	"path/filepath"
)

const passwordScriptName = "user-password.sh"

// manualrun is a tool to test the attendedinstaller in the current terminal window.
// It will simply run the UI and print out the generated Host Configuration.
func main() {
	// Set log-level to warn to show clean up failures
	logger.InitStderrLog()
	logger.SetStderrLogLevel("warn")

	// Example of an image path
	const imagePath = "/mnt/trident_cdrom/images/azure-linux-trident.cosi"

	// Create a temporary directory for config and scripts
	tmpDir, err := os.MkdirTemp("", "trident-manualrun-*")
	if err != nil {
		logger.PanicOnError(fmt.Errorf("failed to create temp dir: %w", err))
	}
	// Clean up the entire temp directory when the program exits
	defer func() {
		if err := os.RemoveAll(tmpDir); err != nil {
			logger.Log.Warnf("Could not delete temp directory: %v", err)
		}
	}()

	hostconfigPath := filepath.Join(tmpDir, "config.yaml")
	passwordScriptPath := filepath.Join(tmpDir, "scripts", passwordScriptName)

	// Run the attended installer
	attendedInstaller, err := attendedinstaller.New(performCalamaresInstallation, imagePath, hostconfigPath)
	if err != nil {
		logger.PanicOnError(err)
	}
	installationQuit, err := attendedInstaller.Run()
	if err != nil {
		logger.Log.Error(err)
	}
	if installationQuit {
		logger.Log.Error("User quit installation")
	}

	displayContent(hostconfigPath, passwordScriptPath)
}

// Shows the contents of the generated Host Configuration and the script to add the user's password
func displayContent(hostconfigPath, passwordScriptPath string) {
	fmt.Println("\n--- Generated Host Configuration: ---")
	if data, err := os.ReadFile(hostconfigPath); err == nil {
		fmt.Println(string(data))
	} else {
		logger.Log.Warnf("Could not read Host Configuration file: %v", err)
	}
	fmt.Println("\n--- Generated Password Script (", passwordScriptPath, ") ---")
	if data, err := os.ReadFile(passwordScriptPath); err == nil {
		fmt.Println(string(data))
	} else {
		logger.Log.Warnf("Could not read generated script to add user's password: %v", err)
	}
}

// A fake calamares installation method.
func performCalamaresInstallation() (err error) {
	logger.Log.Info("Calamares installation requested")
	return
}
