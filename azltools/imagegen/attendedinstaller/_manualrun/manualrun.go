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

// manualrun is a tool to test the attendedinstaller in the current terminal window.
// It will simply run the UI and print out the final config structure's content.
func main() {
	const imagePath = "/mnt/trident_cdrom/images/azure-linux-trident.cosi"
	logger.InitStderrLog()

	// Create a temporary directory for config and scripts
	tmpDir, err := os.MkdirTemp("", "trident-manualrun-*")
	if err != nil {
		logger.PanicOnError(fmt.Errorf("failed to create temp dir: %w", err))
	}
	// Clean up the entire temp directory when the program exits
	defer func() {
		if err := os.RemoveAll(tmpDir); err != nil {
			fmt.Println("(Could not delete temp directory:", err, ")")
		}
	}()

	hostconfigPath := filepath.Join(tmpDir, "config.yaml")
	passwordScriptPath := filepath.Join(tmpDir, "scripts", "user-password.sh")

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

// Shows the contents of the config file and the password script
func displayContent(hostconfigPath, passwordScriptPath string) {
	fmt.Println("\n--- Generated Host Configuration: ---")
	if data, err := os.ReadFile(hostconfigPath); err == nil {
		fmt.Println(string(data))
	} else {
		fmt.Println("(Could not read config file:", err, ")")
	}
	fmt.Println("\n--- Generated Password Script (", passwordScriptPath, ") ---")
	if data, err := os.ReadFile(passwordScriptPath); err == nil {
		fmt.Println(string(data))
	} else {
		fmt.Println("(Could not read password script:", err, ")")
	}
}

// A fake calamares installation method.
func performCalamaresInstallation() (err error) {
	logger.Log.Info("Calamares installation requested")
	return
}
