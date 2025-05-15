// Package helloworld implements a simple hello world scenario and helper.
package helloworld

import (
	"storm/pkg/storm"
	"storm/pkg/storm/core"

	"github.com/sirupsen/logrus"
)

type HelloWorldScenario struct {
	storm.BaseScenario
}

func (s *HelloWorldScenario) Name() string {
	return "hello-world"
}

func (h *HelloWorldScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("myPassingTestCase", func(tc core.TestCase) error {
		logrus.Info("This message will be logged in the test case!")

		// Do something here!
		// ...

		return nil
	})
	return nil
}
