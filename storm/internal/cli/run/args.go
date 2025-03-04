package run

import (
	"fmt"
	"storm/pkg/storm/core"

	"github.com/alecthomas/kong"
)

type runnableKind string

const (
	runnableKindHelper   runnableKind = "helper"
	runnableKindScenario runnableKind = "scenario"
)

func parseExtraArguments(
	suite core.SuiteContext,
	argumented core.Argumented,
	argList []string,
	name string,
	kind runnableKind,
) {
	if argumented.Args() == nil {
		return
	}

	// Create a new parser
	parser, err := kong.New(
		argumented.Args(),
		kong.Name(name),
		kong.Description(fmt.Sprintf("Arguments for %s '%s' in the '%s' suite.", string(kind), name, suite.Name())),
		kong.ConfigureHelp(kong.HelpOptions{NoAppSummary: true}),
	)
	if err != nil {
		suite.Logger().Fatalf("Failed to create parser for %s '%s': %v", string(kind), name, err)
	}

	// If the first argument is '--', we skip it
	var startArg = 0
	if len(argList) != 0 && argList[0] == "--" {
		startArg = 1
	}

	actualArgs := argList[startArg:]

	suite.Logger().Debugf("Parsing extra arguments for %s '%s': %v", string(kind), name, actualArgs)
	_, err = parser.Parse(actualArgs)
	if err != nil {
		suite.Logger().Fatalf("Failed to parse arguments for %s '%s': %v", string(kind), name, err)
	}
}
