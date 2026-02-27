package scenario

import (
	"context"
	"fmt"
	"time"
	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
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
	// Original Host Configuration for this scenario, must NOT be modified
	// directly. Use `config` instead.
	originalConfig hostconfig.HostConfig
	// Parameters specific to this host configuration
	configParams TridentE2EHostConfigParams
	// Test tags derived from the test-selection.yaml configuration. These
	// tags (e.g. "test:base", "test:encryption") control which validation
	// test cases run for this scenario.
	testTags []string

	// Storm scenario arguments, populated when the scenario is executed.
	args struct {
		IsoPath               string             `name:"iso" help:"Path to the ISO to use for OS installation." required:"true"`
		PipelineRun           bool               `name:"pipeline-run" help:"Indicates whether the scenario is being run in a pipeline context. This will, among other things, install dependencies."`
		TestImageDir          string             `short:"i" name:"test-image-dir" help:"Directory containing the test images to use for OS installation." default:"./artifacts/test-image"`
		LogstreamFile         string             `name:"logstream-file" help:"File to write logstream to." default:"logstream-full.log"`
		TracestreamFile       string             `name:"tracestream-file" help:"File to write tracestream to."`
		CertFile              string             `name:"signing-cert" help:"Path to certificate file to inject into VM EFI variables."`
		DumpSshKeyFile        string             `name:"dump-ssh-key" help:"If set, the SSH private key used for VM access will be dumped to the specified file."`
		VmWaitForLoginTimeout int                `name:"vm-wait-for-login-timeout" help:"Time in seconds to wait for the VM to reach login prompt." default:"600"`
		TestRing              testrings.TestRing `name:"test-ring" help:"The test ring in which this scenario is being executed. Defaults to lowest ring for this scenario." env:"TEST_RING"`
	}

	// Runtime variables

	// Stores the SSH private key for VM access
	sshPrivateKey []byte

	// Stores information about the test host once it has been set up
	testHost testHostInfo

	// Stores an open ssh.Client to the test host
	sshClient *ssh.Client

	// Version of the image, used for AB update tests
	version uint

	// Working copy of the host configuration, modified during test execution to
	// reflect changes such as AB updates.
	config hostconfig.HostConfig
}

func NewTridentE2EScenario(
	name string,
	tags []string,
	config hostconfig.HostConfig,
	configParams TridentE2EHostConfigParams,
	hardware HardwareType,
	runtime trident.RuntimeType,
	testRings testrings.TestRingSet,
	testTags []string,
) (*TridentE2EScenario, error) {
	configClone, err := config.Clone()
	if err != nil {
		return nil, fmt.Errorf("failed to clone Host Configuration: %w", err)
	}

	return &TridentE2EScenario{
		name:           name,
		tags:           tags,
		originalConfig: config,
		configParams:   configParams,
		hardware:       hardware,
		runtime:        runtime,
		testRings:      testRings,
		config:         configClone,
		testTags:       testTags,
	}, nil
}

func (s *TridentE2EScenario) Args() any {
	return &s.args
}

func (s *TridentE2EScenario) Setup(storm.SetupCleanupContext) error {
	if s.args.TestRing == testrings.TestRingEmpty {
		// Default to lowest ring
		lowestRing, err := s.testRings.Lowest()
		if err != nil {
			return fmt.Errorf("failed to determine lowest test ring: %w", err)
		}

		s.args.TestRing = lowestRing
	}

	return nil
}

func (s *TridentE2EScenario) Cleanup(storm.SetupCleanupContext) error {
	if s.sshClient != nil {
		s.sshClient.Close()
		s.sshClient = nil
	}

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

// TestTags returns the test selection tags for this scenario (e.g. "test:base",
// "test:encryption"). These are derived from the configuration's
// test-selection.yaml during discovery.
func (s *TridentE2EScenario) TestTags() []string {
	return s.testTags
}

// HasTestTag reports whether the scenario has the given test tag. The tag
// should include the "test:" prefix (e.g. "test:base").
func (s *TridentE2EScenario) HasTestTag(tag string) bool {
	for _, t := range s.testTags {
		if t == tag {
			return true
		}
	}
	return false
}

func (s *TridentE2EScenario) RegisterTestCases(r storm.TestRegistrar) error {
	if s.hardware.IsVM() {
		r.RegisterTestCase("install-vm-deps", s.installVmDependencies)
	}

	r.RegisterTestCase("prepare-hc", s.prepareHostConfig)
	r.RegisterTestCase("setup-test-host", s.setupTestHost)
	r.RegisterTestCase("install-os", s.installOs)
	r.RegisterTestCase("check-trident-ssh", s.checkTridentViaSshAfterInstall)
	r.RegisterTestCase("collect-install-boot-metrics", s.collectInstallBootMetrics)

	if s.HasTestTag("test:base") {
		r.RegisterTestCase("validate-partitions", s.validatePartitions)
		r.RegisterTestCase("validate-users", s.validateUsers)
		r.RegisterTestCase("validate-uefi-fallback", s.validateUefiFallback)
	}

	if s.HasTestTag("test:encryption") {
		r.RegisterTestCase("validate-encryption", s.validateEncryption)
	}

	if s.HasTestTag("test:root_verity") || s.HasTestTag("test:usr_verity") {
		r.RegisterTestCase("validate-verity", s.validateVerity)
	}

	if s.HasTestTag("test:extensions") {
		r.RegisterTestCase("validate-extensions", s.validateExtensions)
	}

	if s.HasTestTag("test:rollback") {
		r.RegisterTestCase("validate-rollback", s.validateRollback)
	}

	if s.originalConfig.HasABUpdate() {
		s.addAbUpdateTests(r, "ab-update-1")
		s.addSplitABUpdateTests(r, "ab-update-split")
	}

	r.RegisterTestCase("publish-logs", s.publishLogs)
	return nil
}

// populateSshClient ensures that `s.sshClient` is populated with a valid SSH client
// connected to the test host. If there is already an open client, it checks if
// it's still valid; if not, it opens a new client.
func (s *TridentE2EScenario) populateSshClient(ctx context.Context) error {
	if s.sshClient != nil {
		logrus.Debug("SSH client already exists, checking validity")
		// There is already an open client, check if it's still valid.
		session, err := s.sshClient.NewSession()
		if err == nil {
			// Session creation succeeded, so the client is still valid. We
			// ignore any error from closing the short-lived session here, as it
			// does not affect the validity of the client itself.
			_ = session.Close()
			logrus.Debug("SSH client is still valid")
			return nil
		}

		// Not valid anymore, close it.
		logrus.Debug("SSH client is no longer valid, reopening")
		s.sshClient.Close()
		s.sshClient = nil
	}

	// If we got here we need to open a new client.

	config := sshutils.SshClientConfig{
		Host:       s.testHost.IPAddress().String(),
		Port:       22,
		User:       testingUsername,
		PrivateKey: s.sshPrivateKey,
		Timeout:    time.Duration(3) * time.Minute,
	}

	logrus.Infof("Creating SSH client to test host at %s", config.FullHost())
	client, err := sshutils.CreateSshClientWithRedial(ctx, time.Second, config)
	if err != nil {
		return fmt.Errorf("failed to create SSH client to test host: %w", err)
	}

	s.sshClient = client

	return nil
}
