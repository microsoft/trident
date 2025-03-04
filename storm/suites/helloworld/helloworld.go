// Package helloworld implements a simple hello world scenario and helper.
package helloworld

import (
	"fmt"
	"storm/pkg/storm"
)

type HelloWorldScenario struct {
	storm.BaseScenario
}

func (s HelloWorldScenario) Name() string {
	return "hello-world"
}

func (s HelloWorldScenario) Run(ctx storm.Context) error {
	ctx.Logger().Infof("Hello from '%s' scenario!", s.Name())
	return nil
}

type HelloWorldHelper struct {
	args struct {
		// Name of the helper
		Name string `arg:"" help:"Name of the helper" default:"default" required:""`
	}
}

func (h HelloWorldHelper) Name() string {
	return "hello-world"
}

func (h *HelloWorldHelper) Args() any {
	return &h.args
}

func (h HelloWorldHelper) Run(ctx storm.HelperContext) error {
	ctx.Logger().Infof("Hello from '%s' helper named '%s'!", h.Name(), h.args.Name)
	tc := ctx.NewTestCase("hello-world")

	tc.Logger().Info("This message will be logged in the test case!")

	tc.Fail("This test case will fail")

	fmt.Println("This message will never be printed!")

	return nil
}
