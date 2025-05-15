package helloworld

import (
	"fmt"
	"storm/pkg/storm"

	"github.com/sirupsen/logrus"
)

// This is a simple implementation of the storm.Helper interface. It is
// meant to be used as an example of how to implement a helper for the storm
// testing framework.
type HelloWorldHelper struct {
	args struct {
		// Name of the helper
		Name string `arg:"" help:"Name of the helper" default:"default" required:""`
	}
}

func (h *HelloWorldHelper) Name() string {
	return "hello-world"
}

func (h *HelloWorldHelper) Args() any {
	return &h.args
}

func (h *HelloWorldHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("myPassingTestCase", h.myPasssingTestCase)
	r.RegisterTestCase("mySkippedTestCase", h.mySkippedTestCase)
	r.RegisterTestCase("myFailingTestCase", h.myFailingTestCase)
	r.RegisterTestCase("myErrorTestCase", h.myErrorTestCase)
	return nil
}

func (h *HelloWorldHelper) myPasssingTestCase(tc storm.TestCase) error {
	// It is recommended to use the logrus logger for logging in your test cases.
	// This will be captured by storm and stored in the test case.
	logrus.Info("This message will be captured by storm and stored in the test case!")

	// If desired, you can also use the standard fmt package to print messages.
	fmt.Println("This message will also be captured!")

	// Do something here!
	// ...
	// time.Sleep(time.Second * 10)

	return nil
}

func (h *HelloWorldHelper) mySkippedTestCase(tc storm.TestCase) error {
	// Skipping will stop execution of this test case here, mark it as skipped
	// and continue with the next test case.
	// time.Sleep(time.Second * 10)
	tc.Skip("Skipping this test case!")
	return nil
}

func (h *HelloWorldHelper) myFailingTestCase(tc storm.TestCase) error {
	logrus.Info("This message will be shown in the failure report!")
	// A failure will stop execution of this test case here, mark it as failed,
	// and stop execution of the entire test suite.
	// time.Sleep(time.Second * 10)
	panic("This test case will fail")
	tc.Fail("This test case will fail")

	// You can also use this handy function to fail a test case from an error!
	// tc.FailFromError()

	fmt.Println("This message will never be printed!")

	return nil
}

func (h *HelloWorldHelper) myErrorTestCase(tc storm.TestCase) error {
	logrus.Info("This test case will never run because we fail before," +
		"but we'll use it to demonstrate error handling.")

	// Storm treats failures an errors differently. Both generally imply that a
	// test case did not achieve the expected outcome, but a failure comes from
	// the object being tested failing, whereas an error comes from the test
	// itself having an error. Failures should generally be actionable and
	// relate to an issue in the product code itself. Error can be transient or
	// just an issue with the test code.
	//
	// For example, imagine a test case that needs to query a server and compare
	// the response against some expected value. If the test does the
	// connection, reads a response and it does NOT match the expected response,
	// that is a test FAILURE. However, what if the test case fails to connect
	// to the server? Depending on the context, it may NOT be desirable to
	// consider that a test failure, but rather an error in the testing itself.
	//
	// Errors, like failures, stop the execution of the entire test suite.
	//
	// To report an ERROR, you can:

	// Call the Error method on the test case. This will stop execution of the
	// test case here and mark it as errored.
	tc.Error(fmt.Errorf("this test case will error"))

	// A panic would also be caught by the test manager and marked as an error!
	// panic("This will be caught by the test manager and marked as an error")

	// Or you can just return an error from the test case like you would do
	// in normal go code.

	return fmt.Errorf("this test case will error")
}
