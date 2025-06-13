// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package main

import (
	"log"
	"os"

	"tridenttools/azltools/internal/exe"
	"tridenttools/azltools/internal/logger"
	"tridenttools/azltools/internal/timestamp"
	"tridenttools/azltools/pkg/osmodifierlib"
	"tridenttools/azltools/pkg/profile"

	"github.com/alecthomas/kingpin/v2"
)

var (
	app = kingpin.New("osmodifier", "Used to modify os")

	configFile    = app.Flag("config-file", "Path of the os modification config file.").String()
	logFlags      = exe.SetupLogFlags(app)
	profFlags     = exe.SetupProfileFlags(app)
	timestampFile = app.Flag("timestamp-file", "File that stores timestamps for this program.").String()
	updateGrub    = app.Flag("update-grub", "Update default GRUB.").Bool()
)

func main() {
	var err error

	kingpin.MustParse(app.Parse(os.Args[1:]))

	logger.InitBestEffort(logFlags)

	prof, err := profile.StartProfiling(profFlags)
	if err != nil {
		logger.Log.Warnf("Could not start profiling: %s", err)
	}
	defer prof.StopProfiler()

	timestamp.BeginTiming("osmodifier", *timestampFile)
	defer timestamp.CompleteTiming()

	// Check if the updateGrub flag is set
	if *updateGrub {
		err := osmodifierlib.ModifyDefaultGrub()
		if err != nil {
			log.Fatalf("update grub failed: %v", err)
		}
	}

	if len(*configFile) > 0 {
		err = modifyImage()
		if err != nil {
			log.Fatalf("OS modification failed: %v", err)
		}
	}
}

func modifyImage() error {
	err := osmodifierlib.ModifyOSWithConfigFile(*configFile)
	if err != nil {
		return err
	}

	return nil
}
