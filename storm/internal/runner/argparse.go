package runner

import (
	"fmt"
	"storm/pkg/storm/core"

	"github.com/alecthomas/kong"
)

func parseExtraArguments(
	suite core.SuiteContext,
	argList []string,
	registrant interface {
		core.TestRegistrantMetadata
		core.Argumented
		core.TestRegistrant
	},
) error {
	if registrant.Args() == nil {
		return nil
	}

	runnableName := registrant.Name()
	runnableType := registrant.RegistrantType()

	// Create a new parser
	parser, err := kong.New(
		registrant.Args(),
		kong.Name(runnableName),
		kong.Description(fmt.Sprintf("Arguments for %s '%s' in the '%s' suite.",
			runnableType,
			runnableName,
			suite.Name(),
		)),
		kong.ConfigureHelp(kong.HelpOptions{NoAppSummary: true}),
	)
	if err != nil {
		suite.Logger().WithError(err).Fatalf(
			"Failed to create parser for %s '%s'",
			runnableType,
			runnableName,
		)
	}

	// If the first argument is '--', we skip it
	var startArg = 0
	if len(argList) != 0 && argList[0] == "--" {
		startArg = 1
	}

	actualArgs := argList[startArg:]

	suite.Logger().Debugf(
		"Parsing extra arguments for %s '%s': %v",
		runnableType,
		runnableName,
		actualArgs,
	)

	_, err = parser.Parse(actualArgs)
	if err != nil {
		return fmt.Errorf(
			"failed to parse arguments for %s '%s': %v",
			runnableType,
			runnableName,
			err,
		)
	}

	return nil
}
