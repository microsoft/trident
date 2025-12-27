package scenario

import (
	"fmt"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/trident"

	"github.com/Jeffail/gabs/v2"
	"github.com/microsoft/storm"
	"gopkg.in/yaml.v3"
)

const (
	defaultNetlaunchListenPort = 4000
)

type TridentE2EHostConfigParams struct {
	// Maximum expected failures for this scenario
	MaxExpectedFailures uint `yaml:"maxExpectedFailures"`

	// Ignore Trident Phonehome failures
	IgnorePhonehomeFailures bool `yaml:"ignorePhonehomeFailures"`

	// Whether this configuration uses a UKI-based image.
	IsUki bool `yaml:"isUki"`
}

type TridentE2EScenario struct {
	// Base scenario from the Storm framework to fulfill the interface.
	storm.BaseScenario

	// Configuration variables. All these fields are guaranteed to be set by the
	// constructor.

	// Name of the scenario
	name string
	// Tags associated with the scenario
	tags []string
	// Hardware type of the scenario
	hardware HardwareType
	// Runtime type of the scenario
	runtime trident.RuntimeType
	// Test rings that this scenario should be run in
	testRings testrings.TestRingSet
	// Host configuration for this scenario
	config *gabs.Container
	// Parameters specific to this host configuration
	configParams TridentE2EHostConfigParams

	// Storm scenario arguments, populated when the scenario is executed.
	args struct {
		IsoPath               string `name:"iso" help:"Path to the ISO to use for OS installation." required:"true"`
		PipelineRun           bool   `name:"pipeline-run" help:"Indicates whether the scenario is being run in a pipeline context. This will, among other things, install dependencies."`
		TestImageDir          string `short:"i" name:"test-image-dir" help:"Directory containing the test images to use for OS installation." default:"./artifacts/test-image"`
		LogstreamFile         string `name:"logstream-file" help:"File to write logstream to." default:"logstream-full.log"`
		TracestreamFile       string `name:"tracestream-file" help:"File to write tracestream to."`
		CertFile              string `name:"signing-cert" help:"Path to certificate file to inject into VM EFI variables."`
		DumpSshKeyFile        string `name:"dump-ssh-key" help:"If set, the SSH private key used for VM access will be dumped to the specified file."`
		VmWaitForLoginTimeout int    `name:"vm-wait-for-login-timeout" help:"Time in seconds to wait for the VM to reach login prompt." default:"600"`
	}

	// Runtime variables

	// Stores the SSH private key for VM access
	sshPrivateKey string

	// Stores information about the test host once it has been set up
	testHost testHostInfo
}

func NewTridentE2EScenario(
	name string,
	tags []string,
	config map[string]interface{},
	configParams TridentE2EHostConfigParams,
	hardware HardwareType,
	runtime trident.RuntimeType,
	testRings testrings.TestRingSet,
) *TridentE2EScenario {
	return &TridentE2EScenario{
		name:         name,
		tags:         tags,
		config:       gabs.Wrap(config),
		configParams: configParams,
		hardware:     hardware,
		runtime:      runtime,
		testRings:    testRings,
	}
}

func (s *TridentE2EScenario) Args() any {
	return &s.args
}

func (s *TridentE2EScenario) Cleanup(storm.SetupCleanupContext) error {
	if s.testHost != nil {
		err := s.testHost.Cleanup()
		if err != nil {
			return fmt.Errorf("failed to clean up test host: %w", err)
		}
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

func (s *TridentE2EScenario) RuntimeType() trident.RuntimeType {
	return s.runtime
}

func (s *TridentE2EScenario) RegisterTestCases(r storm.TestRegistrar) error {
	if s.hardware.IsVM() {
		r.RegisterTestCase("install-vm-deps", s.installVmDependencies)
	}

	r.RegisterTestCase("prepare-hc", s.prepareHostConfig)
	r.RegisterTestCase("setup-test-host", s.setupTestHost)
	r.RegisterTestCase("install-os", s.installOs)
	r.RegisterTestCase("check-trident-ssh", s.checkTridentViaSsh)
	return nil
}

func (s *TridentE2EScenario) renderHostConfiguration() (string, error) {
	out, err := yaml.Marshal(s.config.Data())
	if err != nil {
		return "", fmt.Errorf("failed to marshal host configuration to YAML: %w", err)
	}

	return string(out), nil
}

// func (s *TridentE2EScenario) getSshCliSettings() stormsshconfig.SshCliSettings {
// 	return stormsshconfig.SshCliSettings{
// 		Host:           s.testHost.IPAddress(),
// 		Port:           22,
// 		User:           testingUsername,
// 		PrivateKeyData: []byte(s.sshPrivateKey),
// 		Timeout:        10,
// 	}
// }
