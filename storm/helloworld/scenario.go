// Package helloworld implements a simple hello world scenario and helper.
package helloworld

import (
	"storm"
	"storm/pkg/storm/core"

	"github.com/sirupsen/logrus"
)

type HelloWorldScenario struct {
	// You can embed storm.BaseScenario to get the default implementation of the Scenario interface.
	// This is useful if you are not using most methods.
	// storm.BaseScenario
}

// Args implements core.Scenario.
func (s *HelloWorldScenario) Args() any {
	return nil
}

// Setup implements core.Scenario.
func (s *HelloWorldScenario) Setup(core.SetupCleanupContext) error {
	logrus.Info("Setup called for HelloWorldScenario")
	return nil
}

// Cleanup implements core.Scenario.
func (s *HelloWorldScenario) Cleanup(core.SetupCleanupContext) error {
	logrus.Info("Cleanup called for HelloWorldScenario")
	return nil
}

// RequiredFiles implements core.Scenario.
func (s *HelloWorldScenario) RequiredFiles() []string {
	return nil
}

// StagePaths implements core.Scenario.
func (s *HelloWorldScenario) StagePaths() []string {
	return nil
}

// Tags implements core.Scenario.
func (s *HelloWorldScenario) Tags() []string {
	return nil
}

// Type implements core.Scenario.
func (s *HelloWorldScenario) Name() string {
	return "hello-world"
}

// Description implements core.Scenario.
func (h *HelloWorldScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("myPassingTestCase", func(tc storm.TestCase) error {
		logrus.Info("This message will be logged in the test case!")

		// Do something here!
		// ...

		return nil
	})
	return nil
}
