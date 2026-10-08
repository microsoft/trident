package tests

import (
	"fmt"
	"strings"

	stormaclconfig "tridenttools/storm/aclagent/utils/config"
	stormssh "tridenttools/storm/utils/ssh"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func CheckDeployment(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return stormvm.CheckDeployment(vmConfig, testConfig.ExpectedInitialVolume)
}

func DeployVM(testConfig stormaclconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
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

// readVmProductUUID reads the VM's hardware product UUID
// (/sys/class/dmi/id/product_uuid). Every fake Node seeded for
// trident-acl-agent (stormproxies.NewSeedNode) must carry this exact value
// as its status.nodeInfo.systemUUID: when TRIDENT_ACL_AGENT_VALIDATE_NODE_UUID
// is set, trident-acl-agent's NodeClient::get_node/watch_node
// (crates/trident-acl-agent/src/annotations/k8s.rs) compare the two and
// treat a non-empty mismatch as the Node not existing. An empty
// systemUUID is NOT treated as a mismatch (verification is skipped), so
// only a WRONG systemUUID here would make the agent never find its Node.
func readVmProductUUID(cfg stormvmconfig.VMConfig, vmIP string) (string, error) {
	out, err := stormssh.SshCommandCombinedOutput(cfg, vmIP, "sudo cat /sys/class/dmi/id/product_uuid")
	if err != nil {
		return "", fmt.Errorf("failed to read VM product uuid: %w", err)
	}
	return strings.TrimSpace(out), nil
}
