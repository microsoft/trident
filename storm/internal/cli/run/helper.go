package run

import (
	"storm/internal/runner"
	"storm/pkg/storm/core"
)

type HelperCmd struct {
	Helper     string   `arg:"" name:"helper" help:"Name of the helper to run"`
	Watch      bool     `short:"w" help:"Watch the output of the helper live"`
	LogDir     *string  `short:"l" help:"Optional directory to save logs to. Will be created if it does not exist." type:"path"`
	HelperArgs []string `arg:"" passthrough:"all" help:"Arguments to pass to the helper, you may use '--' to force passthrough." optional:""`
}

func (cmd *HelperCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Infof("Running helper '%s'", cmd.Helper)

	helper := suite.Helper(cmd.Helper)

	return runner.RegisterAndRunTests(suite, helper, cmd.HelperArgs, cmd.Watch, cmd.LogDir)
}
