package tests

import (
	"fmt"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormnetlisten "tridenttools/storm/utils/netlisten"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func CheckDeployment(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.CheckDeployment(vmConfig, testConfig.ExpectedVolume)
}

func DeployVM(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		logrus.Tracef("Deploying VM on QEMU platform with name '%s'", vmConfig.VMConfig.Name)
		if err := vmConfig.QemuConfig.DeployQemuVM(vmConfig.VMConfig.Name, testConfig.ArtifactsDir, testConfig.OutputPath, testConfig.Verbose); err != nil {
			return fmt.Errorf("failed to deploy qemu vm: %w", err)
		}
	}
	return nil
}

func CleanupVM(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		if err := vmConfig.AzureConfig.CleanupAzureVM(); err != nil {
			return fmt.Errorf("failed to cleanup Azure VM: %w", err)
		}
	}
	stormnetlisten.KillUpdateServer(testConfig.FileServerPort)
	return nil
}
