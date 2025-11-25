package vmip

import (
	"fmt"
	"tridenttools/storm/servicing/utils/config"
)

func GetVmIP(cfg config.ServicingConfig) (string, error) {
	allIps, err := GetAllVmIPAddresses(cfg)
	if err != nil {
		return "", fmt.Errorf("failed to get all VM IP addresses: %w", err)
	}
	if len(allIps) == 0 {
		return "", fmt.Errorf("no IP addresses found for VM '%s'", cfg.VMConfig.Name)
	}
	return allIps[0], nil
}

func GetAllVmIPAddresses(cfg config.ServicingConfig) ([]string, error) {
	if cfg.VMConfig.Platform == config.PlatformQEMU {
		ips, err := cfg.QemuConfig.GetAllVmIPAddresses(cfg.VMConfig.Name)
		if err != nil {
			return nil, fmt.Errorf("failed to get QEMU VM IP addresses: %w", err)
		}
		return ips, nil
	} else if cfg.VMConfig.Platform == config.PlatformAzure {
		ips, err := cfg.AzureConfig.GetAllVmIPAddresses(cfg.VMConfig.Name, cfg.TestConfig.BuildId)
		if err != nil {
			return nil, fmt.Errorf("failed to get Azure VM IP addresses: %w", err)
		}
		return ips, nil
	}
	return nil, fmt.Errorf("unknown platform: %s", cfg.VMConfig.Platform)
}
