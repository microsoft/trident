package tests

import (
	"fmt"
	"tridenttools/storm/servicing/utils/config"
	"tridenttools/storm/servicing/utils/vmip"

	"github.com/sirupsen/logrus"
)

func CheckDeployment(cfg config.ServicingConfig) error {
	logrus.Tracef("Get VM IP address(es)")
	vmIPs, err := vmip.GetAllVmIPAddresses(cfg)
	if err != nil {
		return fmt.Errorf("failed to get VM IP addresses: %w", err)
	}
	if len(vmIPs) == 0 {
		return fmt.Errorf("no VM IP addresses found")
	}
	logrus.Infof("Found VM IP address(es): %v", vmIPs)

	// Help diagnose https://dev.azure.com/mariner-org/ECF/_workitems/edit/11273 and
	// fail explicitly if multiple IPs are found
	if cfg.VMConfig.Platform == config.PlatformQEMU {
		if len(vmIPs) > 1 {
			logrus.Errorf("Multiple IPs found, expected only one: %v", vmIPs)
			return fmt.Errorf("multiple IPs found, expected only one: %v", vmIPs)
		}
	}

	logrus.Tracef("Check if VM is reachable and has expected active volume")
	if err := checkActiveVolume(cfg.VMConfig, vmIPs[0], cfg.TestConfig.ExpectedVolume); err != nil {
		return fmt.Errorf("failed to check active volume '%s': %w", cfg.TestConfig.ExpectedVolume, err)
	}

	return nil
}

func DeployVM(cfg config.ServicingConfig) error {
	if cfg.VMConfig.Platform == config.PlatformQEMU {
		logrus.Tracef("Deploying VM on QEMU platform with name '%s'", cfg.VMConfig.Name)
		if err := cfg.QemuConfig.DeployQemuVM(cfg.VMConfig.Name, cfg.TestConfig.ArtifactsDir, cfg.TestConfig.OutputPath, cfg.TestConfig.Verbose); err != nil {
			return fmt.Errorf("failed to deploy qemu vm: %w", err)
		}
	} else if cfg.VMConfig.Platform == config.PlatformAzure {
		logrus.Tracef("Deploying VM on Azure platform with name '%s'", cfg.VMConfig.Name)
		if err := cfg.AzureConfig.DeployAzureVM(cfg.VMConfig.Name, cfg.VMConfig.User, cfg.TestConfig.BuildId); err != nil {
			return fmt.Errorf("failed to deploy azure vm: %w", err)
		}
	}
	return nil
}

func CleanupVM(cfg config.ServicingConfig) error {
	if cfg.VMConfig.Platform == config.PlatformAzure {
		if err := cfg.AzureConfig.CleanupAzureVM(); err != nil {
			return fmt.Errorf("failed to cleanup Azure VM: %w", err)
		}
	} else if cfg.VMConfig.Platform == config.PlatformQEMU {
		if err := cfg.QemuConfig.CleanupQemuVM(cfg.VMConfig.Name); err != nil {
			return fmt.Errorf("failed to cleanup QEMU VM: %w", err)
		}
	}
	killUpdateServer(cfg.TestConfig.UpdatePortA)
	killUpdateServer(cfg.TestConfig.UpdatePortB)
	return nil
}
