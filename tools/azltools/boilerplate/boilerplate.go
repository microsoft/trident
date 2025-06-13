// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// A boilerplate for Azure Linux go tools

package main

import (
	"os"

	"tridenttools/azltools/boilerplate/hello"
	"tridenttools/azltools/internal/exe"
	"tridenttools/azltools/internal/logger"
	"tridenttools/azltools/internal/timestamp"

	"github.com/alecthomas/kingpin/v2"
)

var (
	app = kingpin.New("boilerplate", "A sample golang tool for Azure Linux.")

	logFlags      = exe.SetupLogFlags(app)
	timestampFile = app.Flag("timestamp-file", "File that stores timestamps for this program.").String()
)

func main() {
	app.Version(exe.ToolkitVersion)
	kingpin.MustParse(app.Parse(os.Args[1:]))

	logger.InitBestEffort(logFlags)

	timestamp.BeginTiming("boilerplate", *timestampFile)
	defer timestamp.CompleteTiming()

	logger.Log.Info(hello.World())
}
