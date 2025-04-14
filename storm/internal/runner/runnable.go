package runner

import "storm/pkg/storm/core"

type runnableInstance struct {
	core.Argumented
	core.TestRegistrant
}

func (ri *runnableInstance) RegistrantType() core.RegistrantType {
	if _, ok := ri.TestRegistrant.(core.Scenario); ok {
		return core.RegistrantTypeScenario
	}

	if _, ok := ri.TestRegistrant.(core.Helper); ok {
		return core.RegistrantTypeHelper
	}

	panic("unknown runnable type")

}
