// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"fmt"
	"os"
	"os/signal"
	"path/filepath"

	"installer/imagegen/attendedinstaller"
	"installer/imagegen/configuration"
	"installer/imagegen/diskutils"
	"installer/internal/exe"
	"installer/internal/file"
	"installer/internal/logger"

	"github.com/alecthomas/kingpin/v2"
	"golang.org/x/sys/unix"
)

var (
	app              = kingpin.New("liveinstaller", "A tool to install Azure Linux using Trident with an interactive terminal UI.")
	unattended       = app.Flag("unattended", "Use the unattended flag for the installer without user interaction. Host Configuration Template must be provided").Bool()
	imagesDir        = app.Flag("images-dir", "Path to the directory containing OS images for the target system.").String()
	templateFile     = app.Flag("template-file", "Path to the YAML template file used to generate the Host Configuration for unattended installation.").Default("").String()
	hostConfigOutput = app.Flag("host-config-output", "Output path where the generated Host Configuration file will be saved.").Default("/etc/trident/config.yaml").String()
	logFlags         = exe.SetupLogFlags(app)
)

func handleCtrlC(signals chan os.Signal) {
	<-signals
	logger.Log.Error("Installation in progress, please wait until finished.")
}

func main() {
	kingpin.MustParse(app.Parse(os.Args[1:]))
	logger.InitBestEffort(logFlags)

	// Prevent a SIGINT (Ctrl-C) from stopping liveinstaller while an installation is in progress.
	// It is the responsibility of the installer's user interface to handle quit requests from the user.
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, unix.SIGINT)
	go handleCtrlC(signals)

	installFunc := installerFactory(*unattended)
	installationQuit, err := installFunc()
	if installationQuit {
		logger.Log.Error("User quit installation")
		// Return a non-zero exit code to drop the user to shell
		os.Exit(1)
	}

	if err == nil {
		// Execute Trident
		logger.Log.Infof("Executing Trident with Host Configuration: %s", *hostConfigOutput)
		err = unix.Exec("/usr/bin/trident",
			[]string{"trident", "install", *hostConfigOutput},
			os.Environ())
	}
	logger.PanicOnError(err)
}

// Returns the correct function to execute the selected installation process
func installerFactory(unattended bool) (installFunc func() (bool, error)) {
	if unattended {
		logger.Log.Info("The unattended flag is set, using unattended installation")
		installFunc = func() (bool, error) {
			return unattendedInstall()
		}
	} else {
		logger.Log.Infof("Proceeding with attended installation")
		installFunc = func() (bool, error) {
			return terminalUIAttendedInstall()
		}
	}

	return
}

// Runs the terminal UI for attended installation
func terminalUIAttendedInstall() (installationQuit bool, err error) {
	// Convert to absolute path
	*hostConfigOutput, err = filepath.Abs(*hostConfigOutput)
	if err != nil {
		return
	}

	// Check if the images directory exists and is a directory
	exists, err := file.DirExists(*imagesDir)
	if err != nil {
		return
	}
	if !exists {
		err = fmt.Errorf("images directory not found : '%s'. "+
			"Please specify a valid directory containing OS images using the --images-dir flag", *imagesDir)
		return
	}
	// Convert images directory to absolute path
	*imagesDir, err = filepath.Abs(*imagesDir)
	if err != nil {
		return
	}

	// Initialize the attended installer
	attendedInstaller, err := attendedinstaller.New(*imagesDir, *hostConfigOutput)
	if err != nil {
		return
	}

	// Execute installation UI
	installationQuit, err = attendedInstaller.Run()
	return
}

// Runs unattended installation using a Host Configuration template
func unattendedInstall() (installationQuit bool, err error) {
	installationQuit = false

	// Check if template is provided and exists
	exists, err := file.PathExists(*templateFile)
	if err != nil {
		return
	}
	if !exists {
		err = fmt.Errorf("template file not found: '%s'. "+
			"Please specify a valid YAML template file using the --template-file flag", *templateFile)
		return
	}

	// Set disk path for installation
	devicePath, err := getDevicePath()
	if err != nil {
		return
	}

	// Convert template path to absolute path and render the Host Configuration
	templatePath, err := filepath.Abs(*templateFile)
	if err != nil {
		return
	}
	generatedHostConfigPath, err := configuration.RenderHostConfigurationUnattended(templatePath, devicePath)
	if err != nil {
		return
	}
	*hostConfigOutput = generatedHostConfigPath

	return
}

// getDevicePath returns the device path for installation by selecting the first available disk
// Mirrors the behavior of autopartitionwidget's default selection.
func getDevicePath() (string, error) {
	// Get all system devices
	systemDevices, err := diskutils.SystemBlockDevices()
	if err != nil {
		return "", fmt.Errorf("failed to get system devices: %w", err)
	}
	if len(systemDevices) == 0 {
		return "", fmt.Errorf("no system devices found")
	}

	return systemDevices[0].DevicePath, nil
}
