package vm

import (
	"fmt"

	stormtridentactivevolume "tridenttools/storm/utils/trident/activevolume"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func CheckDeployment(vmConfig stormvmconfig.AllVMConfig, expectedVolume string) error {
	logrus.Tracef("Get VM IP address(es)")
	vmIPs, err := GetAllVmIPAddresses(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP addresses: %w", err)
	}
	if len(vmIPs) == 0 {
		return fmt.Errorf("no VM IP addresses found")
	}
	logrus.Infof("Found VM IP address(es): %v", vmIPs)

	// Help diagnose https://dev.azure.com/mariner-org/ECF/_workitems/edit/11273 and
	// fail explicitly if multiple IPs are found
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		if len(vmIPs) > 1 {
			logrus.Errorf("Multiple IPs found, expected only one: %v", vmIPs)
			return fmt.Errorf("multiple IPs found, expected only one: %v", vmIPs)
		}
	}

	logrus.Tracef("Check if VM is reachable and has expected active volume")
	if err := stormtridentactivevolume.CheckActiveVolume(vmConfig.VMConfig, vmIPs[0], expectedVolume); err != nil {
		return fmt.Errorf("failed to check active volume '%s': %w", expectedVolume, err)
	}

	return nil
}
