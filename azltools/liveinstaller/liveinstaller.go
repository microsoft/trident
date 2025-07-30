// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"bufio"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"regexp"
	"strings"

	"azltools/imagegen/attendedinstaller"
	"azltools/imagegen/configuration"
	"azltools/internal/exe"
	"azltools/internal/jsonutils"
	"azltools/internal/logger"
	"azltools/internal/shell"

	"github.com/alecthomas/kingpin/v2"
	"golang.org/x/sys/unix"
)

var (
	app = kingpin.New("liveinstaller", "A tool to download a provided list of packages into a given directory.")

	unattended     = app.Flag("unattended", "Use the unattended installer without user interaction.").Bool()
	imagePath      = app.Flag("image-path", "Path to the OS image for the target system.").Required().String()
	hostconfigPath = app.Flag("host-config", "Path to the host configuration file.").Default("/etc/trident/config.yaml").String()
	logFlags       = exe.SetupLogFlags(app)
)

// Every valid mouse event handler will follow the format:
// H: Handlers=eventX mouseX
var mouseEventHandlerRegex = regexp.MustCompile(`^H:\s+Handlers=(\w+)\s+mouse\d+`)

// Used for Calamares based installation
type imagerArguments struct {
	imagerTool       string
	configFile       string
	buildDir         string
	baseDirPath      string
	emitProgress     bool
	logFile          string
	logLevel         string
	repoSnapshotTime string
}

func handleCtrlC(signals chan os.Signal) {
	<-signals
	logger.Log.Error("Installation in progress, please wait until finished.")
}

func main() {
	app.Version(exe.ToolkitVersion)
	kingpin.MustParse(app.Parse(os.Args[1:]))
	logger.InitBestEffort(logFlags)

	// Prevent a SIGINT (Ctr-C) from stopping liveinstaller while an installation is in progress.
	// It is the responsibility of the installer's user interface (terminal installer or Calamares) to handle quit requests from the user.
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

	// Trident needs to eject the disk:
	// ejectDisk()
}

// Returns the correct funtion to execute the selected installation process
func installerFactory(unattended bool) (installFunc func() (bool, error)) {
	isAttended := false

	// Determine if the attended installer should be shown
	if unattended {
		logger.Log.Info("`unattended` flag set, using unattended installation")
		isAttended = false
	} else {
		logger.Log.Infof("Unattended installation is currently not supported. Using attended installation.")
		isAttended = true
	}

	if isAttended {
		installFunc = func() (bool, error) {
			return terminalUIAttendedInstall()
		}
	}

	return
}

// Runs the terminal UI for attended installation
func terminalUIAttendedInstall() (installationQuit bool, err error) {
	// Initialize the attended installer
	attendedInstaller, err := attendedinstaller.New(
		// Calamares based installation
		func() (err error) {
			return calamaresInstall()
		}, *imagePath, *hostconfigPath)

	if err != nil {
		return
	}

	// Execute installation UI
	installationQuit, err = attendedInstaller.Run()
	return
}

// This function will be replaced in Trident
func ejectDisk() (err error) {
	logger.Log.Info("Ejecting CD-ROM.")
	const squashErrors = false
	program := "eject"
	commandArgs := []string{
		"--cdrom",
		"--force",
	}
	err = shell.ExecuteLive(squashErrors, program, commandArgs...)

	if err != nil {
		// If there was an error ejecting the CD-ROM, assume this is a USB installation and prompt the user
		// to remove the USB device before rebooting.
		logger.Log.Info("==================================================================================")
		logger.Log.Info("Installation Complete. Please Remove USB installation media and reboot if present.")
		logger.Log.Info("==================================================================================")
	}
	return
}

// Calamares based installation
func calamaresInstall() (err error) {
	const (
		squashErrors = false
		calamaresDir = "/etc/calamares"
	)
	args := imagerArguments{}
	args.emitProgress = true
	args.configFile = filepath.Join(calamaresDir, "unattended_config.json")

	launchScript := filepath.Join(calamaresDir, "mariner-install.sh")
	skuDir := filepath.Join(calamaresDir, "azurelinux-skus")

	bootType := configuration.SystemBootType()
	logger.Log.Infof("Boot type detected: %s", bootType)

	mouseHandlers, err := findMouseHandlers()
	if err != nil {
		// Not finding a mouse isn't fatal as the installer can instead be driven with
		// a keyboard only.
		logger.Log.Warnf("No mouse detected: %v", err)
	}

	logger.Log.Infof("Using (%s) for mouse input", mouseHandlers)
	newEnv := append(shell.CurrentEnvironment(), fmt.Sprintf("QT_QPA_EVDEV_MOUSE_PARAMETERS=%s", mouseHandlers))
	shell.SetEnvironment(newEnv)

	// Generate the files needed for calamares
	err = os.MkdirAll(skuDir, os.ModePerm)
	if err != nil {
		return
	}

	err = generateCalamaresLaunchScript(launchScript, args)
	if err != nil {
		return
	}

	// Generate the partial JSONs for SKUs
	err = generateCalamaresSKUs(skuDir, bootType)
	if err != nil {
		return
	}

	return shell.ExecuteLive(squashErrors, "calamares", "-platform", "linuxfb")
}

// Failing in azl-installer.iso
func findMouseHandlers() (handlers string, err error) {
	const (
		deviceHandlerFile   = "/proc/bus/input/devices"
		eventPrefix         = "/dev/input"
		handlerDelimiter    = ":"
		absoluteInputEvents = "abs"
		eventMatchGroup     = 1
	)

	devicesFile, err := os.Open(deviceHandlerFile)
	if err != nil {
		return
	}
	defer devicesFile.Close()

	// Gather a list of all mouse event handlers from the devices file
	eventHandlers := []string{}
	scanner := bufio.NewScanner(devicesFile)
	for scanner.Scan() {
		matches := mouseEventHandlerRegex.FindStringSubmatch(scanner.Text())
		if len(matches) == 0 {
			continue
		}

		eventPath := filepath.Join(eventPrefix, matches[eventMatchGroup])
		eventHandlers = append(eventHandlers, eventPath)
	}

	err = scanner.Err()
	if err != nil {
		return
	}

	if len(eventHandlers) == 0 {
		err = fmt.Errorf("no mouse handler detected")
		return
	}

	// Add the the absolute input modifier to the handler list as mouse events are absolute.
	// QT's default behavior is to take in relative events.
	eventHandlers = append(eventHandlers, absoluteInputEvents)

	// Join all mouse event handlers together so they all function inside QT
	handlers = strings.Join(eventHandlers, handlerDelimiter)

	return
}

func generateCalamaresLaunchScript(launchScriptPath string, args imagerArguments) (err error) {
	const executionPerm = 0755

	// Generate the script calamares will invoke to install
	scriptFile, err := os.OpenFile(launchScriptPath, os.O_CREATE|os.O_RDWR, executionPerm)
	if err != nil {
		return
	}
	defer scriptFile.Close()

	logger.Log.Infof("Generating install script (%s)", launchScriptPath)
	program, commandArgs := formatImagerCommand(args)

	scriptFile.WriteString("#!/bin/bash\n")
	scriptFile.WriteString(fmt.Sprintf("%s %s", program, strings.Join(commandArgs, " ")))
	scriptFile.WriteString("\n")

	return
}

func generateCalamaresSKUs(skuDir, bootType string) (err error) {
	// Parse template config
	templateConfig, err := configuration.Load("/root/installer/attended_config.json")
	if err != nil {
		return
	}

	// Generate JSON snippets for each SKU
	for _, sysConfig := range templateConfig.SystemConfigs {
		sysConfig.BootType = bootType
		err = generateSingleCalamaresSKU(sysConfig, skuDir)
		if err != nil {
			return
		}
	}

	return
}

func generateSingleCalamaresSKU(sysConfig configuration.SystemConfig, skuDir string) (err error) {
	skuFilePath := filepath.Join(skuDir, sysConfig.Name+".json")
	logger.Log.Infof("Generating SKU option (%s)", skuFilePath)

	// Write the individual system config to a file.
	return jsonutils.WriteJSONFile(skuFilePath, sysConfig)
}

func formatImagerCommand(args imagerArguments) (program string, commandArgs []string) {
	program = args.imagerTool

	commandArgs = []string{
		"--live-install",
		fmt.Sprintf("--input=%s", args.configFile),
		fmt.Sprintf("--build-dir=%s", args.buildDir),
		fmt.Sprintf("--base-dir=%s", args.baseDirPath),
		fmt.Sprintf("--log-file=%s", args.logFile),
		fmt.Sprintf("--log-level=%s", args.logLevel),
		fmt.Sprintf("--repo-snapshot-time=%s", args.repoSnapshotTime),
	}

	if args.emitProgress {
		commandArgs = append(commandArgs, "--emit-progress")
	}

	return
}
