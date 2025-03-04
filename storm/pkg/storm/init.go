package storm

import (
	"storm/pkg/storm/core"
	"storm/pkg/storm/suite"
)

type Scenario = core.Scenario
type BaseScenario = core.BaseScenario

type Helper = core.Helper
type BaseHelper = core.BaseHelper

type Context = core.ScenarioContext
type HelperContext = core.HelperContext

type TestCase = core.TestCase

type LoggerProvider = core.LoggerProvider

// Creates a new suite with the given name.
func CreateSuite(name string) suite.StormSuite {
	return suite.CreateSuite(name)
}
