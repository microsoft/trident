package tests

import (
	"fmt"

	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

// genericQemuImagePattern must match stormvmqemu.QemuConfig.ImagePattern's
// own default tag exactly - see the doc comment on aclImagePattern below.
const genericQemuImagePattern = `^trident-vm-.*-testimage\.qcow2$`

// aclImagePattern scopes DeployVM's QEMU image lookup to this scenario's own
// base image. QemuConfig.ImagePattern's shared default matches every
// trident-vm-*-testimage.qcow2 in the artifacts directory; FindFile errors
// out if more than one file matches, so a local artifacts/ directory holding
// another storm scenario's test image would otherwise make deploy-vm fail
// here. Only substituted when the caller left ImagePattern at that shared
// default - an explicit CLI override (--image-pattern) always wins.
const aclImagePattern = `^trident-vm-acl-agent-testimage\.qcow2$`

func CheckDeployment(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.CheckDeployment(vmConfig, testConfig.ExpectedInitialVolume)
}

func DeployVM(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		logrus.Tracef("Deploying VM on QEMU platform with name '%s'", vmConfig.VMConfig.Name)
		qemuConfig := vmConfig.QemuConfig
		if qemuConfig.ImagePattern == genericQemuImagePattern {
			qemuConfig.ImagePattern = aclImagePattern
		}
		if err := qemuConfig.DeployQemuVM(vmConfig.VMConfig.Name, testConfig.ArtifactsDir, testConfig.OutputPath, testConfig.Verbose); err != nil {
			return fmt.Errorf("failed to deploy qemu vm: %w", err)
		}
	} else if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		logrus.Tracef("Deploying VM on Azure platform with name '%s'", vmConfig.VMConfig.Name)
		if err := vmConfig.AzureConfig.DeployAzureVM(vmConfig.VMConfig.Name, vmConfig.VMConfig.User); err != nil {
			return fmt.Errorf("failed to deploy azure vm: %w", err)
		}
	} else {
		return fmt.Errorf("unsupported VM platform '%s'", vmConfig.VMConfig.Platform)
	}
	return nil
}

func CleanupVM(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		if err := vmConfig.AzureConfig.CleanupAzureVM(); err != nil {
			return fmt.Errorf("failed to cleanup Azure VM: %w", err)
		}
	} else if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		if err := vmConfig.QemuConfig.CleanupQemuVM(vmConfig.VMConfig.Name); err != nil {
			return fmt.Errorf("failed to cleanup QEMU VM: %w", err)
		}
	} else {
		return fmt.Errorf("unsupported VM platform '%s'", vmConfig.VMConfig.Platform)
	}
	return nil
}
