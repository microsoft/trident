package aclagent

import (
	"fmt"
	"os"
	"path/filepath"

	stormtests "tridenttools/storm/aclagent/tests"
	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormvmazure "tridenttools/storm/utils/vm/azure"
	stormvmconfig "tridenttools/storm/utils/vm/config"
	stormvmqemu "tridenttools/storm/utils/vm/qemu"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type TridentAclAgentScenario struct {
	args TridentAclAgentScenarioArgs
}

type TridentAclAgentScenarioArgs struct {
	stormaclconfig.TestConfig `embed:""`
	stormvmconfig.VMConfig    `embed:""`
	stormvmqemu.QemuConfig    `embed:""`
	stormvmazure.AzureConfig  `embed:""`
	TestCaseToRun             string `help:"Name of the test case to run. If not specified, all test cases will be run." default:"all"`
}

func (s *TridentAclAgentScenario) Name() string                             { return "aclagent" }
func (s *TridentAclAgentScenario) Args() any                                { return &s.args }
func (s *TridentAclAgentScenario) Tags() []string                           { return []string{} }
func (s *TridentAclAgentScenario) StagePaths() []string                     { return []string{} }
func (s *TridentAclAgentScenario) RequiredFiles() []string                  { return nil }
func (s TridentAclAgentScenario) Setup(ctx storm.SetupCleanupContext) error { return nil }

func (s *TridentAclAgentScenario) Cleanup(ctx storm.SetupCleanupContext) error {
	if s.args.TestConfig.ForceCleanup {
		_ = stormtests.CleanupVM(s.args.TestConfig, stormvmconfig.AllVMConfig{VMConfig: s.args.VMConfig, QemuConfig: s.args.QemuConfig, AzureConfig: s.args.AzureConfig})
	}
	return nil
}

func (s *TridentAclAgentScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("deploy-vm", s.deployVm)
	r.RegisterTestCase("check-deployment", s.checkDeployment)
	r.RegisterTestCase("run-ab-update", s.runABUpdate)
	r.RegisterTestCase("collect-logs", s.collectLogs)
	r.RegisterTestCase("cleanup-vm", s.cleanupVm)
	return nil
}

func (s *TridentAclAgentScenario) runTestCase(tc storm.TestCase, testFunc func(stormaclconfig.TestConfig, stormvmconfig.AllVMConfig) error) error {
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

func (s *TridentAclAgentScenario) deployVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.DeployVM)
}
func (s *TridentAclAgentScenario) checkDeployment(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.CheckDeployment)
}
func (s *TridentAclAgentScenario) runABUpdate(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.RunABUpdate)
}
func (s *TridentAclAgentScenario) collectLogs(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.FetchLogs)
}
func (s *TridentAclAgentScenario) cleanupVm(tc storm.TestCase) error {
	return s.runTestCase(tc, stormtests.CleanupVM)
}
