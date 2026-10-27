package rollback

import (
	"os"
	"path/filepath"

	stormrollbacktests "tridenttools/storm/rollback/tests"
	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormvmazure "tridenttools/storm/utils/vm/azure"
	stormvmconfig "tridenttools/storm/utils/vm/config"
	stormvmqemu "tridenttools/storm/utils/vm/qemu"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
)

type TridentRollbackScenario struct {
	args TridentRollbackScenarioArgs
}

type TridentRollbackScenarioArgs struct {
	stormrollbackconfig.TestConfig `embed:""`
	stormvmconfig.VMConfig         `embed:""`
	stormvmqemu.QemuConfig         `embed:""`
	stormvmazure.AzureConfig       `embed:""`
}

func (s *TridentRollbackScenario) Name() string {
	return "rollback"
}

func (s *TridentRollbackScenario) Args() any {
	return &s.args
}

func (s *TridentRollbackScenario) Tags() []string {
	return []string{}
}

func (s *TridentRollbackScenario) StagePaths() []string {
	return []string{}
}

func (s *TridentRollbackScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("prepare-extensions", s.prepareExtensions)
	r.RegisterTestCase("prepare-qcow2", s.prepareQcow2)
	r.RegisterTestCase("deploy-vm", s.deployVm)
	r.RegisterTestCase("check-deployment", s.checkDeployment)
	r.RegisterTestCase("multi-rollback", s.multiRollback)
	r.RegisterTestCase("skip-to-ab-rollback", s.skipToAbRollback)
	r.RegisterTestCase("split-rollback", s.splitRollback)
	r.RegisterTestCase("collect-logs", s.collectLogs)
	r.RegisterTestCase("cleanup-vm", s.cleanupVm)
	return nil
}

func (s *TridentRollbackScenario) RequiredFiles() []string {
	return nil
}

func (s *TridentRollbackScenario) Setup(ctx storm.SetupCleanupContext) error {
	profile, err := s.args.TestConfig.ApplyFlavor()
	if err != nil {
		return err
	}

	// A flavor that cannot enroll the test signing keys into firmware must not
	// run with secure boot, even when the caller asked for it.
	s.args.QemuConfig.SecureBoot = s.args.QemuConfig.SecureBoot && profile.SupportsSecureBoot

	logrus.Infof(
		"Rollback flavor %q: uki=%t secure-boot=%t skip-extensions=%t skip-runtime-updates=%t skip-netplan=%t skip-manual-rollbacks=%t",
		s.args.TestConfig.Flavor,
		s.args.TestConfig.Uki,
		s.args.QemuConfig.SecureBoot,
		s.args.TestConfig.SkipExtensionTesting,
		s.args.TestConfig.SkipRuntimeUpdates,
		s.args.TestConfig.SkipNetplanRuntimeTesting,
		s.args.TestConfig.SkipManualRollbacks,
	)

	return nil
}

func (s *TridentRollbackScenario) Cleanup(ctx storm.SetupCleanupContext) error {
	if s.args.TestConfig.ForceCleanup {
		// Best effort to clean up azure resources in case there was a failure
		stormrollbacktests.CleanupVM(
			s.args.TestConfig,
			stormvmconfig.AllVMConfig{
				VMConfig:    s.args.VMConfig,
				QemuConfig:  s.args.QemuConfig,
				AzureConfig: s.args.AzureConfig,
			})
	}
	return nil
}

func (s *TridentRollbackScenario) runTestCase(tc storm.TestCase, testFunc func(stormrollbackconfig.TestConfig, stormvmconfig.AllVMConfig) error) error {
	logrus.Infof("Running test case '%s'", tc.Name())
	// create test-specific output directory
	testCaseSpecificConfig := s.args.TestConfig
	testCaseSpecificConfig.OutputPath = s.args.TestConfig.OutputPath
	if testCaseSpecificConfig.OutputPath != "" {
		testCaseSpecificConfig.OutputPath = filepath.Join(testCaseSpecificConfig.OutputPath, tc.Name())
		if err := os.MkdirAll(testCaseSpecificConfig.OutputPath, 0755); err != nil {
			tc.FailFromError(err)
		}
	}
	err := testFunc(
		testCaseSpecificConfig,
		stormvmconfig.AllVMConfig{
			VMConfig:    s.args.VMConfig,
			QemuConfig:  s.args.QemuConfig,
			AzureConfig: s.args.AzureConfig,
		})
	if err != nil {
		logrus.Infof("test case '%s' failed", tc.Name())
		tc.FailFromError(err)
	}
	logrus.Infof("test case '%s' passed", tc.Name())
	return nil
}

func (s *TridentRollbackScenario) prepareExtensions(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.PrepareExtensions)
}

func (s *TridentRollbackScenario) prepareQcow2(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.PrepareQcow2)
}

func (s *TridentRollbackScenario) deployVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.DeployVM)
}

func (s *TridentRollbackScenario) checkDeployment(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.CheckDeployment)
}

func (s *TridentRollbackScenario) multiRollback(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.MultiRollbackTest)
}

func (s *TridentRollbackScenario) skipToAbRollback(tc storm.TestCase) error {
	if s.args.SkipRuntimeUpdates {
		tc.Skip("Skipping skip-to-ab rollback test due to SkipRuntimeUpdates being true")
	}
	if s.args.SkipManualRollbacks {
		tc.Skip("Skipping skip-to-ab rollback test due to SkipManualRollbacks being true")
	}
	return s.runTestCase(tc, stormrollbacktests.SkipToAbRollbackTest)
}

func (s *TridentRollbackScenario) splitRollback(tc storm.TestCase) error {
	if s.args.SkipManualRollbacks {
		tc.Skip("Skipping split rollback test due to SkipManualRollbacks being true")
	}
	return s.runTestCase(tc, stormrollbacktests.SplitRollbackTest)
}

func (s *TridentRollbackScenario) collectLogs(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.FetchLogs)
}

func (s *TridentRollbackScenario) cleanupVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.CleanupVM)
}
