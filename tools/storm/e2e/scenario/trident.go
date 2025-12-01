package scenario

import (
	"fmt"
	"tridenttools/storm/e2e/testrings"

	"github.com/microsoft/storm"
)

type TridentE2EScenario struct {
	storm.BaseScenario
	name      string
	tags      []string
	config    map[string]interface{}
	hardware  HardwareType
	runtime   RuntimeType
	testRings testrings.TestRingSet
	args      struct {
		PipelineRun bool `long:"pipeline-run" help:"Indicates whether the scenario is being run in a pipeline context."`
	}

	// Stores the SSH private key for VM access
	sshPrivateKey string

	// Stores information about the test host once it has been set up
	testHost testHostInfo
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

func (s *TridentE2EScenario) Args() any {
	return &s.args
}

func (s *TridentE2EScenario) Cleanup(storm.SetupCleanupContext) error {
	err := s.testHost.Cleanup()
	if err != nil {
		return fmt.Errorf("failed to clean up test host: %w", err)
	}

	return nil
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
	r.RegisterTestCase("install-vm-deps", s.installVmDependencies)
	r.RegisterTestCase("prepare-hc", s.prepareHostConfig)
	r.RegisterTestCase("setup-vm", s.setupTestHost)
	return nil
}
