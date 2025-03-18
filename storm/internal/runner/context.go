package runner

import (
	"storm/pkg/storm/core"

	"github.com/sirupsen/logrus"
)

type runnableContext struct {
	runnableMeta core.RunnableMetadata
	logger       *logrus.Logger
	testCreator  core.TestCaseCreator
}

func (rc *runnableContext) Name() string {
	return rc.runnableMeta.Name()
}

func (rc *runnableContext) RunnableType() core.RunnableType {
	return rc.runnableMeta.RunnableType()
}

func (rc *runnableContext) Logger() *logrus.Logger {
	return rc.logger
}

func (rc *runnableContext) NewTestCase(name string) core.TestCase {
	return rc.testCreator.NewTestCase(name)
}
