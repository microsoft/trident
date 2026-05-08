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

	// Log a warning if multiple IPs are still found after stabilization.
	// Previously this was a hard failure for diagnostic purposes (ADO#11273).
	// Root cause: libvirt's dnsmasq DHCP server can assign multiple leases to
	// a single VM MAC when duplicate DHCPDISCOVER packets are sent at boot.
	// See: https://libvirt.org/html/libvirt-libvirt-domain.html#virDomainInterfaceAddresses
	// GetAllVmIPAddresses now handles stabilization and returns a single IP,
	// but we keep the warning for observability in case it still occurs.
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
		if len(vmIPs) > 1 {
			logrus.Warnf("Multiple IPs found (expected one): %v — proceeding with %s", vmIPs, vmIPs[0])
		}
	}

	logrus.Tracef("Check if VM is reachable and has expected active volume")
	if err := stormtridentactivevolume.CheckActiveVolume(vmConfig.VMConfig, vmIPs[0], expectedVolume); err != nil {
		return fmt.Errorf("failed to check active volume '%s': %w", expectedVolume, err)
	}

	return nil
}
