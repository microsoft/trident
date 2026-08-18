package aksagent

import (
	"fmt"
	"os"
	"path/filepath"

	stormtests "tridenttools/storm/aksagent/tests"
	stormaksconfig "tridenttools/storm/aksagent/utils/config"
	stormvmazure "tridenttools/storm/utils/vm/azure"
	stormvmconfig "tridenttools/storm/utils/vm/config"
	stormvmqemu "tridenttools/storm/utils/vm/qemu"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type TridentAksAgentScenario struct {
	args TridentAksAgentScenarioArgs
}

type TridentAksAgentScenarioArgs struct {
	stormaksconfig.TestConfig `embed:""`
	stormvmconfig.VMConfig    `embed:""`
	stormvmqemu.QemuConfig    `embed:""`
	stormvmazure.AzureConfig  `embed:""`
	TestCaseToRun             string `help:"Name of the test case to run. If not specified, all test cases will be run." default:"all"`
}

func (s *TridentAksAgentScenario) Name() string                             { return "aksagent" }
func (s *TridentAksAgentScenario) Args() any                                { return &s.args }
func (s *TridentAksAgentScenario) Tags() []string                           { return []string{} }
func (s *TridentAksAgentScenario) StagePaths() []string                     { return []string{} }
func (s *TridentAksAgentScenario) RequiredFiles() []string                  { return nil }
func (s TridentAksAgentScenario) Setup(ctx storm.SetupCleanupContext) error { return nil }

func (s *TridentAksAgentScenario) Cleanup(ctx storm.SetupCleanupContext) error {
	if s.args.TestConfig.ForceCleanup {
		_ = stormtests.CleanupVM(s.args.TestConfig, stormvmconfig.AllVMConfig{VMConfig: s.args.VMConfig, QemuConfig: s.args.QemuConfig, AzureConfig: s.args.AzureConfig})
	}
	return nil
}

func (s *TridentAksAgentScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("deploy-vm", s.deployVm)
	r.RegisterTestCase("check-deployment", s.checkDeployment)
	r.RegisterTestCase("run-ab-update", s.runABUpdate)
	r.RegisterTestCase("run-rollback", s.runRollback)
	r.RegisterTestCase("collect-logs", s.collectLogs)
	r.RegisterTestCase("cleanup-vm", s.cleanupVm)
	return nil
}

func (s *TridentAksAgentScenario) runTestCase(tc storm.TestCase, testFunc func(stormaksconfig.TestConfig, stormvmconfig.AllVMConfig) error) error {
	if tc.Name() != s.args.TestCaseToRun && s.args.TestCaseToRun != "all" {
		tc.Skip(fmt.Sprintf("Test case '%s' does not align to TestCaseToRun '%s'", tc.Name(), s.args.TestCaseToRun))
		return nil
	}
	logrus.Infof("Running test case '%s'", tc.Name())
	testCaseSpecificConfig := s.args.TestConfig
	if testCaseSpecificConfig.OutputPath != "" {
		testCaseSpecificConfig.OutputPath = filepath.Join(testCaseSpecificConfig.OutputPath, tc.Name())
		if err := os.MkdirAll(testCaseSpecificConfig.OutputPath, 0o755); err != nil {
			tc.FailFromError(err)
		}
	}
	if err := testFunc(testCaseSpecificConfig, stormvmconfig.AllVMConfig{VMConfig: s.args.VMConfig, QemuConfig: s.args.QemuConfig, AzureConfig: s.args.AzureConfig}); err != nil {
		logrus.Infof("test case '%s' failed", tc.Name())
		tc.FailFromError(err)
	}
	logrus.Infof("test case '%s' passed", tc.Name())
	return nil
}

func (s *TridentAksAgentScenario) deployVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.DeployVM)
}
func (s *TridentAksAgentScenario) checkDeployment(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.CheckDeployment)
}
func (s *TridentAksAgentScenario) runABUpdate(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.RunABUpdate)
}
func (s *TridentAksAgentScenario) runRollback(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.RunRollback)
}
func (s *TridentAksAgentScenario) collectLogs(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.FetchLogs)
}
func (s *TridentAksAgentScenario) cleanupVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.CleanupVM)
}
