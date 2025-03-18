package run

import (
	"storm/internal/runner"
	"storm/pkg/storm/core"

	"github.com/sirupsen/logrus"
)

type ScenarioCmd struct {
	Scenario     string   `arg:"" name:"scenario" help:"Name of the scenario to run"`
	ScenarioArgs []string `arg:"" passthrough:"all" help:"Arguments to pass to the scenario, you may use '--' to force passthrough." optional:""`
}

type ScenarioRunnerContext struct {
	name     string
	logger   *logrus.Logger
	reporter core.TestCaseCreator
}

func (rc *ScenarioRunnerContext) Name() string {
	return rc.name
}

func (rc *ScenarioRunnerContext) RunnableType() core.RunnableType {
	return core.RunnableTypeScenario
}

func (rc *ScenarioRunnerContext) Logger() *logrus.Logger {
	return rc.logger
}

func (rc *ScenarioRunnerContext) NewTestCase(name string) core.TestCase {
	return rc.reporter.NewTestCase(name)
}

func (cmd *ScenarioCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Infof("Running scenario '%s'", cmd.Scenario)

	scenario := suite.Scenario(cmd.Scenario)

	return runner.RunRunnable(suite, scenario, cmd.ScenarioArgs)
}
