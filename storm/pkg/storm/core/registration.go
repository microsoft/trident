package core

type RegistrantType int

const (
	RegistrantTypeScenario RegistrantType = iota
	RegistrantTypeHelper
)

func (t RegistrantType) String() string {
	switch t {
	case RegistrantTypeScenario:
		return "scenario"
	case RegistrantTypeHelper:
		return "helper"
	default:
		return "unknown"
	}
}

type TestRegistrant interface {
	Named
	RegisterTestCases(r TestRegistrar) error
}

type TestRegistrantMetadata interface {
	Named

	// Returns the type of the registrant.
	RegistrantType() RegistrantType
}

type TestCaseFunction = func(TestCase) error

type TestRegistrar interface {
	// Register a test case with the given name. The name is used to identify
	// the test case in the test suite. The name should be unique within the
	// test suite. Test names MUST be accepted by the regular expression
	// `^[a-zA-Z0-9_]+$`.
	RegisterTestCase(name string, runner TestCaseFunction)
}
