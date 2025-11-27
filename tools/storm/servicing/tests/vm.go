package tests

import (
	"fmt"

	stormsvcconfig "tridenttools/storm/servicing/utils/config"
	stormnetlisten "tridenttools/storm/utils/netlisten"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func CheckDeployment(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.CheckDeployment(vmConfig, testConfig.ExpectedVolume)
}

func DeployVM(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		logrus.Tracef("Deploying VM on QEMU platform with name '%s'", vmConfig.VMConfig.Name)
		if err := vmConfig.QemuConfig.DeployQemuVM(vmConfig.VMConfig.Name, testConfig.ArtifactsDir, testConfig.OutputPath, testConfig.Verbose); err != nil {
			return fmt.Errorf("failed to deploy qemu vm: %w", err)
		}
	} else if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		logrus.Tracef("Deploying VM on Azure platform with name '%s'", vmConfig.VMConfig.Name)
		if err := vmConfig.AzureConfig.DeployAzureVM(vmConfig.VMConfig.Name, vmConfig.VMConfig.User); err != nil {
			return fmt.Errorf("failed to deploy azure vm: %w", err)
		}
	}
	return nil
}

func CleanupVM(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		if err := vmConfig.AzureConfig.CleanupAzureVM(); err != nil {
			return fmt.Errorf("failed to cleanup Azure VM: %w", err)
		}
	} else if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		if err := vmConfig.QemuConfig.CleanupQemuVM(vmConfig.VMConfig.Name); err != nil {
			return fmt.Errorf("failed to cleanup QEMU VM: %w", err)
		}
	}
	stormnetlisten.KillUpdateServer(testConfig.UpdatePortA)
	stormnetlisten.KillUpdateServer(testConfig.UpdatePortB)
	return nil
}
