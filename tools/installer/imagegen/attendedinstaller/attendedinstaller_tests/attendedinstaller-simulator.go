// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"fmt"
	"installer/imagegen/attendedinstaller"
	"installer/internal/logger"
	"os"
	"path/filepath"

	"github.com/alecthomas/kingpin/v2"
)

const userScriptName = "user-password.sh"

var (
	app           = kingpin.New("attendedinstaller-simulator", "A tool to simulate an attended installation process and generate a Host Configuration. No actual installation is performed.")
	hostconfigDir = app.Flag("output-dir", "Directory where the generated Host Configuration file will be saved.").Default("").String()
	logLevel      = app.Flag("log-level", "Set the log level.").Default("warn").String()
)

// attendedinstaller_simulator is a tool to run the attendedinstaller UI in the current terminal window.
// It will run the UI and print out the generated Host Configuration without performing an actual installation.
func main() {
	kingpin.MustParse(app.Parse(os.Args[1:]))

	// Set log-level
	logger.InitStderrLog()
	logger.SetStderrLogLevel(*logLevel)

	// Create a temporary directory for test setup
	tmpDir, err := os.MkdirTemp("", "trident-attendedinstaller-simulator-*")
	if err != nil {
		logger.PanicOnError(fmt.Errorf("failed to create temp dir: %w", err))
	}
	// Clean up the entire temp directory when the program exits
	defer func() {
		if err := os.RemoveAll(tmpDir); err != nil {
			logger.Log.Warnf("Could not delete temp directory: %v", err)
		}
	}()

	// Create a fake .cosi files for testing
	imagesDir, err := prepareTestDirectory(tmpDir)
	if err != nil {
		logger.PanicOnError(fmt.Errorf("failed to create fake COSI files: %w", err))
	}

	hostconfigPath := filepath.Join(tmpDir, "config.yaml")
	userScriptPath := filepath.Join(tmpDir, "scripts", userScriptName)

	attendedInstaller, err := attendedinstaller.New(imagesDir, hostconfigPath)
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

	displayContent(hostconfigPath, userScriptPath)

	if *hostconfigDir != "" {
		err = os.MkdirAll(*hostconfigDir, 0755)
		if err != nil {
			logger.Log.Warnf("Failed to create directory %s: %v", *hostconfigDir, err)
		} else {
			targetPath := filepath.Join(*hostconfigDir, "config.yaml")
			err = os.Rename(hostconfigPath, targetPath)
			if err != nil {
				logger.Log.Errorf("Failed to move config file to %s: %v", targetPath, err)
			} else {
				logger.Log.Infof("Host configuration saved to: %s", targetPath)
			}
		}
	}

}

// Shows the contents of the generated Host Configuration and the script to add the user's password
func displayContent(hostconfigPath, userScriptPath string) {
	fmt.Println("\n--- Generated Host Configuration: ---")
	if data, err := os.ReadFile(hostconfigPath); err == nil {
		fmt.Println(string(data))
	} else {
		logger.Log.Warnf("Could not read Host Configuration file: %v", err)
	}
	fmt.Println("\n--- Generated Password Script (", userScriptPath, ") ---")
	if data, err := os.ReadFile(userScriptPath); err == nil {
		fmt.Println(string(data))
	} else {
		logger.Log.Warnf("Could not read generated script to add user's password: %v", err)
	}
}

// prepareTestDirectory creates the necessary files and directories for testing the attendedinstaller
// and returns the path to the images directory containing the fake .cosi files.
func prepareTestDirectory(testDir string) (string, error) {
	// Create a directory for images and a fake .cosi file
	imagesDir := filepath.Join(testDir, "images")
	err := os.MkdirAll(imagesDir, 0755)
	if err != nil {
		return "", fmt.Errorf("failed to create images directory: %w", err)
	}

	fakeFiles := []string{
		filepath.Join(imagesDir, "azure-linux-core.cosi"),
		filepath.Join(imagesDir, "azure-linux-full.cosi"),
	}

	for _, filePath := range fakeFiles {
		err := os.WriteFile(filePath, []byte("# Fake COSI file for testing\n"), 0644)
		if err != nil {
			return "", fmt.Errorf("failed to create fake COSI file %s: %w", filePath, err)
		}
	}

	return imagesDir, nil
}
