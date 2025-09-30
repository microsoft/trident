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
	"azltools/internal/exe"
	"azltools/internal/logger"

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
	attendedInstaller, err := attendedinstaller.New(*imagePath, *hostconfigPath)

	if err != nil {
		return
	}

	// Execute installation UI
	installationQuit, err = attendedInstaller.Run()
	return
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
