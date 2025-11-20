package vm

import (
	"fmt"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func GetVmIP(vmConfig stormvmconfig.AllVMConfig) (string, error) {
	allIps, err := GetAllVmIPAddresses(vmConfig)
	if err != nil {
		return "", fmt.Errorf("failed to get all VM IP addresses: %w", err)
	}
	if len(allIps) == 0 {
		return "", fmt.Errorf("no IP addresses found for VM '%s'", vmConfig.VMConfig.Name)
	}
	return allIps[0], nil
}

func GetAllVmIPAddresses(vmConfig stormvmconfig.AllVMConfig) ([]string, error) {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		ips, err := vmConfig.QemuConfig.GetAllVmIPAddresses(vmConfig.VMConfig.Name)
		if err != nil {
			return nil, fmt.Errorf("failed to get QEMU VM IP addresses: %w", err)
		}
		return ips, nil
	} else if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		ips, err := vmConfig.AzureConfig.GetAllVmIPAddresses(vmConfig.VMConfig.Name)
		if err != nil {
			return nil, fmt.Errorf("failed to get Azure VM IP addresses: %w", err)
		}
		return ips, nil
	}
	return nil, fmt.Errorf("unknown platform: %s", vmConfig.VMConfig.Platform)
}
