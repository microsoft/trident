// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"azltools/imagegen/attendedinstaller"
	"azltools/internal/logger"
)

// manualrun is a tool to test the attendedinstaller in the current terminal window.
// It will simply run the UI and print out the final config structure's content.
func main() {
	const (
		assetDirPath      = "./"
		configFileDirPath = "./"
	)

	logger.InitStderrLog()

	attendedInstaller, err := attendedinstaller.New(performCalamaresInstallation)
	logger.PanicOnError(err)

	installationQuit, err := attendedInstaller.Run()
	if installationQuit {
		logger.Log.Error("User quit installation")
		return
	}
	logger.PanicOnError(err)
}

// A fake calamares installation method.
func performCalamaresInstallation() (err error) {
	logger.Log.Info("Calamares installation requested")
	return
}
