package run

import (
	"storm/internal/runner"
	"storm/pkg/storm/core"

	"github.com/sirupsen/logrus"
)

type HelperCmd struct {
	Helper     string   `arg:"" name:"helper" help:"Name of the helper to run"`
	HelperArgs []string `arg:"" passthrough:"all" help:"Arguments to pass to the helper, you may use '--' to force passthrough." optional:""`
}

type HelperRunnerContext struct {
	name     string
	logger   *logrus.Logger
	reporter core.TestCaseCreator
}

func (rc *HelperRunnerContext) Name() string {
	return rc.name
}

func (rc *HelperRunnerContext) RunnableType() core.RunnableType {
	return core.RunnableTypeHelper
}

func (rc *HelperRunnerContext) Logger() *logrus.Logger {
	return rc.logger
}

func (rc *HelperRunnerContext) NewTestCase(name string) core.TestCase {
	return rc.reporter.NewTestCase(name)
}

func (cmd *HelperCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Infof("Running helper '%s'", cmd.Helper)

	helper := suite.Helper(cmd.Helper)

	return runner.RunRunnable(suite, helper, cmd.HelperArgs)
}
