package runner

import "storm/pkg/storm/core"

type runnableInstance struct {
	core.ArgumentedRunnable
}

func (ri *runnableInstance) RunnableType() core.RunnableType {
	if _, ok := ri.ArgumentedRunnable.(core.Scenario); ok {
		return core.RunnableTypeScenario
	}

	if _, ok := ri.ArgumentedRunnable.(core.Helper); ok {
		return core.RunnableTypeHelper
	}

	panic("unknown runnable type")

}
