package run

import (
	"storm/pkg/storm/core"

	"github.com/sirupsen/logrus"
)

type HelperCmd struct {
	Helper     string   `arg:"" name:"helper" help:"Name of the helper to run"`
	HelperArgs []string `arg:"" passthrough:"all" help:"Arguments to pass to the helper, you may use '--' to force passthrough." optional:""`
}

type HelperRunnerContext struct {
	logger   *logrus.Logger
	reporter core.TestCaseCreator
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

	return runTestRunnable(
		suite,
		helper,
		cmd.HelperArgs,
		cmd.Helper,
		runnableKindHelper,
		func(testManager core.TestCaseCreator) error {
			return helper.Run(&HelperRunnerContext{
				logger:   log,
				reporter: testManager,
			})
		},
	)
}
