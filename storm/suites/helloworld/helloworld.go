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

func (h HelloWorldHelper) Run(ctx storm.Context) error {
	ctx.Logger().Infof("Hello from '%s' helper named '%s'!", h.Name(), h.args.Name)
	tc := ctx.NewTestCase("myPassingTestCase")

	tc.Logger().Info("This message will be logged in the test case!")

	// Do something here!
	// ...

	tc.Pass()

	// Fun fact: if you don't call `tc.Pass()` the test case will be
	// automatically marked as passed(unless an error is returned!) when the
	// test manager is closed or when you start a new test case.

	tc = ctx.NewTestCase("mySkippedTestCase")

	// Do something here!
	// ...

	tc.SkipAndContinue("This test is not needed!")

	tc = ctx.NewTestCase("myFailingTestCase")

	// A panic would also be caught by the test manager and marked as an error!
	// panic("This will be caught by the test manager and marked as an error")

	tc.Fail("This test case will fail")

	fmt.Println("This message will never be printed!")

	tc = ctx.NewTestCase("myErrorTestCase")
	tc.Logger().Info("This test case will never run because we fail before.")

	// Assuming we got here okay and we were to return an error, it would get attached to
	// the last test case, `myErrorTestCase`, as we never closed it with a pass or fail.

	return nil
}
