package rollback

import (
	"fmt"
	"os"
	"path/filepath"

	"tridenttools/storm/rollback/tests"
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
	r.RegisterTestCase("deploy-vm", s.deployVm)
	r.RegisterTestCase("check-deployment", s.checkDeployment)
	r.RegisterTestCase("update-and-rollback", s.updateAndRollback)
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

func (h *TridentRollbackScenario) Cleanup(ctx storm.SetupCleanupContext) error {
	if h.args.TestConfig.ForceCleanup {
		// Best effort to clean up azure resources in case there was a failure
		stormrollbacktests.CleanupVM(
			h.args.TestConfig,
			stormvmconfig.AllVMConfig{
				VMConfig:    h.args.VMConfig,
				QemuConfig:  h.args.QemuConfig,
				AzureConfig: h.args.AzureConfig,
			})
	}
	return nil
}

func (h *TridentRollbackScenario) runTestCase(tc storm.TestCase, testFunc func(stormrollbackconfig.TestConfig, stormvmconfig.AllVMConfig) error) error {
	if tc.Name() != h.args.TestCaseToRun && h.args.TestCaseToRun != "all" {
		tc.Skip(fmt.Sprintf("Test case '%s' does not align to TestCaseToRun '%s'", tc.Name(), h.args.TestCaseToRun))
	} else {
		logrus.Infof("Running test case '%s'", tc.Name())
		// create test-specific output directory
		testCaseSpecificConfig := h.args.TestConfig
		testCaseSpecificConfig.OutputPath = h.args.TestConfig.OutputPath
		if testCaseSpecificConfig.OutputPath != "" {
			testCaseSpecificConfig.OutputPath = filepath.Join(testCaseSpecificConfig.OutputPath, tc.Name())
			if err := os.MkdirAll(testCaseSpecificConfig.OutputPath, 0755); err != nil {
				tc.FailFromError(err)
			}
		}
		err := testFunc(
			testCaseSpecificConfig,
			stormvmconfig.AllVMConfig{
				VMConfig:    h.args.VMConfig,
				QemuConfig:  h.args.QemuConfig,
				AzureConfig: h.args.AzureConfig,
			})
		if err != nil {
			logrus.Infof("test case '%s' failed", tc.Name())
			tc.FailFromError(err)
		}
		logrus.Infof("test case '%s' passed", tc.Name())
	}
	return nil

}

func (h *TridentRollbackScenario) deployVm(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.DeployVM)
}

func (h *TridentRollbackScenario) checkDeployment(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.CheckDeployment)
}

func (h *TridentRollbackScenario) updateAndRollback(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.RollbackTest)
}

func (h *TridentRollbackScenario) collectLogs(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.FetchLogs)
}

func (h *TridentRollbackScenario) cleanupVm(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.CleanupVM)
}
