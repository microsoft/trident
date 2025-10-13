// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"fmt"
	"os"
	"os/signal"

	"installer/imagegen/attendedinstaller"
	"installer/imagegen/configuration"
	"installer/imagegen/diskutils"
	"installer/internal/exe"
	"installer/internal/logger"

	"github.com/alecthomas/kingpin/v2"
	"golang.org/x/sys/unix"
)

var (
	app = kingpin.New("liveinstaller", "A tool to install Azure Linux using Trident with an interactive terminal UI.")

	unattended             = app.Flag("unattended", "Use the unattended installer without user interaction.").Bool()
	imagesDir              = app.Flag("images-dir", "Path to the directory containing OS images for the target system.").Required().String()
	hostConfigTemplatePath = app.Flag("host-config-template", "Path to the Host Configuration template file for unattended installation.").Default("").String()
	hostConfigOutputPath   = app.Flag("host-config-output", "Output path where the Host Configuration file will be created.").Default("/etc/trident/config.yaml").String()
	logFlags               = exe.SetupLogFlags(app)
)

func handleCtrlC(signals chan os.Signal) {
	<-signals
	logger.Log.Error("Installation in progress, please wait until finished.")
}

func main() {
	kingpin.MustParse(app.Parse(os.Args[1:]))
	logger.InitBestEffort(logFlags)

	// Prevent a SIGINT (Ctr-C) from stopping liveinstaller while an installation is in progress.
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
	// Initialize the attended installer
	attendedInstaller, err := attendedinstaller.New(*imagesDir, *hostConfigOutputPath)

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

	if *hostConfigTemplatePath == "" {
		err = fmt.Errorf("unattended installation requires a Host Configuration template. Use --host-config-template flag")
		return
	}

	logger.Log.Infof("Using Host Configuration template: %s", *hostConfigTemplatePath)

	// Set disk path for installation
	devicePath, err := getDevicePath()
	if err != nil {
		return
	}

	// Render the Host Configuration using the provided template file
	generatedHostConfigPath, err := configuration.RenderHostConfigurationUnattended(*hostConfigTemplatePath, devicePath)
	if err != nil {
		return
	}

	logger.Log.Infof("Host Configuration created at: %s", generatedHostConfigPath)

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
