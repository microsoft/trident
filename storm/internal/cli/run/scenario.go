package run

import (
	"storm/internal/runner"
	"storm/pkg/storm/core"
)

type ScenarioCmd struct {
	Scenario     string   `arg:"" name:"scenario" help:"Name of the scenario to run"`
	Watch        bool     `short:"w" help:"Watch the output of the scenario live"`
	LogDir       *string  `short:"l" help:"Optional directory to save logs to. Will be created if it does not exist." type:"path"`
	ScenarioArgs []string `arg:"" passthrough:"all" help:"Arguments to pass to the scenario, you may use '--' to force passthrough." optional:""`
}

func (cmd *ScenarioCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Infof("Running scenario '%s'", cmd.Scenario)

	scenario := suite.Scenario(cmd.Scenario)

	return runner.RegisterAndRunTests(suite, scenario, cmd.ScenarioArgs, cmd.Watch, cmd.LogDir)
}
