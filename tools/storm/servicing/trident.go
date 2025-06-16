package servicing

import (
	"fmt"
	"os"
	"path/filepath"
	"tridenttools/storm/servicing/tests"
	"tridenttools/storm/servicing/utils/azure"
	"tridenttools/storm/servicing/utils/config"
	"tridenttools/storm/servicing/utils/qemu"

	"storm"

	"github.com/sirupsen/logrus"
)

type TridentServicingScenario struct {
	args TridentServicingScenarioArgs
}

type TridentServicingScenarioArgs struct {
	config.TestConfig `embed:""`
	config.VMConfig   `embed:""`
	qemu.QemuConfig   `embed:""`
	azure.AzureConfig `embed:""`
	TestCaseToRun     string `help:"Name of the test case to run. If not specified, all test cases will be run." default:"all"`
}

func (s *TridentServicingScenario) Name() string {
	return "servicing"
}

func (s *TridentServicingScenario) Args() any {
	return &s.args
}

func (s *TridentServicingScenario) Tags() []string {
	return []string{}
}

func (s *TridentServicingScenario) StagePaths() []string {
	return []string{}
}

func (s *TridentServicingScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("publish-sig-image", s.publishSigImage)
	r.RegisterTestCase("deploy-vm", s.deployVm)
	r.RegisterTestCase("check-deployment", s.checkDeployment)
	r.RegisterTestCase("update-loop", s.updateLoop)
	r.RegisterTestCase("rollback", s.rollback)
	r.RegisterTestCase("collect-logs", s.collectLogs)
	r.RegisterTestCase("cleanup-vm", s.cleanupVm)
	return nil
}

func (s *TridentServicingScenario) RequiredFiles() []string {
	return nil
}

func (s TridentServicingScenario) Setup(ctx storm.SetupCleanupContext) error {
	return nil
}

func (h *TridentServicingScenario) Cleanup(ctx storm.SetupCleanupContext) error {
	if h.args.TestConfig.ForceCleanup {
		// Best effort to clean up azure resources in case there was a failure
		tests.CleanupVM(config.ServicingConfig{
			TestConfig:  h.args.TestConfig,
			VMConfig:    h.args.VMConfig,
			QemuConfig:  h.args.QemuConfig,
			AzureConfig: h.args.AzureConfig,
		})
	}
	return nil
}

func (h *TridentServicingScenario) runTestCase(tc storm.TestCase, testFunc func(config.ServicingConfig) error) error {
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
		err := testFunc(config.ServicingConfig{
			TestConfig:  testCaseSpecificConfig,
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

func (h *TridentServicingScenario) deployVm(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.DeployVM)
}

func (h *TridentServicingScenario) checkDeployment(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.CheckDeployment)
}

func (h *TridentServicingScenario) updateLoop(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.UpdateLoop)
}

func (h *TridentServicingScenario) rollback(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.Rollback)
}

func (h *TridentServicingScenario) collectLogs(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.FetchLogs)
}

func (h *TridentServicingScenario) cleanupVm(tc storm.TestCase) error {
	return h.runTestCase(tc, tests.CleanupVM)
}

func (h *TridentServicingScenario) publishSigImage(tc storm.TestCase) error {
	if h.args.Platform != config.PlatformAzure {
		tc.Skip("Test case 'publish-sig-image' is only applicable for Azure platform")
		return nil // No action needed for non-Azure platforms
	}

	return h.runTestCase(tc, tests.PublishSigImage)
}
