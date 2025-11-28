package scenario

import (
	"fmt"
	"tridenttools/storm/e2e/testrings"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
)

type TridentE2EScenario struct {
	storm.BaseScenario
	name      string
	tags      []string
	config    map[string]interface{}
	hardware  HardwareType
	runtime   RuntimeType
	testRings testrings.TestRingSet
}

func NewTridentE2EScenario(name string, tags []string, config map[string]interface{}, hardware HardwareType, runtime RuntimeType, testRings testrings.TestRingSet) *TridentE2EScenario {
	return &TridentE2EScenario{
		name:      name,
		tags:      tags,
		config:    config,
		hardware:  hardware,
		runtime:   runtime,
		testRings: testRings,
	}
}

func (s *TridentE2EScenario) TestRings() testrings.TestRingSet {
	return s.testRings
}

func (s *TridentE2EScenario) Name() string {
	return s.name
}

func (s *TridentE2EScenario) Tags() []string {
	return s.tags
}

func (s *TridentE2EScenario) HardwareType() HardwareType {
	return s.hardware
}

func (s *TridentE2EScenario) RuntimeType() RuntimeType {
	return s.runtime
}

func (s *TridentE2EScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("run", s.Run)
	return nil
}

func (s TridentE2EScenario) Run(tc storm.TestCase) error {
	logrus.Infof("Hello from '%s'!", s.Name())

	logrus.Infof("Hardware Type: %s", s.hardware)
	logrus.Infof("Runtime Type: %s", s.runtime)
	logrus.Infof("Configuration: ")
	fmt.Println(s.config)

	// TODO: Implement the actual scenario logic here

	return nil
}
