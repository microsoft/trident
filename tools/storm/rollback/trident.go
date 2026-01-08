package rollback

import (
	"fmt"
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
	TestCaseToRun                  string `help:"Name of the test case to run. If not specified, all test cases will be run." default:"all"`
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

func (s TridentRollbackScenario) Setup(ctx storm.SetupCleanupContext) error {
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
	if tc.Name() != s.args.TestCaseToRun && s.args.TestCaseToRun != "all" {
		tc.Skip(fmt.Sprintf("Test case '%s' does not align to TestCaseToRun '%s'", tc.Name(), s.args.TestCaseToRun))
	} else {
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
	}
	return nil

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
	return s.runTestCase(tc, stormrollbacktests.SkipToAbRollbackTest)
}

func (s *TridentRollbackScenario) splitRollback(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.SplitRollbackTest)
}

func (s *TridentRollbackScenario) collectLogs(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.FetchLogs)
}

func (s *TridentRollbackScenario) cleanupVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormrollbacktests.CleanupVM)
}
