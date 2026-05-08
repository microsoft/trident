// Package provides libvirt/qemu VM utility functions.
package qemu

import (
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	stormutils "tridenttools/storm/utils"
	stormfile "tridenttools/storm/utils/file"

	"github.com/digitalocean/go-libvirt"
	"github.com/sirupsen/logrus"
)

type QemuConfig struct {
	SecureBoot bool   `help:"Enable secure boot for the VM" default:"false"`
	SerialLog  string `help:"Path to the serial log file" default:"/tmp/trident-vm-verity-test.log"`
}

func (cfg QemuConfig) DeployQemuVM(vmName string, artifactsDir string, outputPath string, verbose bool) error {
	logrus.Tracef("Deploying VM on QEMU platform with name '%s'", vmName)

	// Destroy and undefine any existing VM
	if err := cfg.deleteLibvirtDomain(vmName); err != nil {
		return fmt.Errorf("failed to delete existing domain '%s': %w", vmName, err)
	}

	// Find image file
	imageFile, err := stormfile.FindFile(artifactsDir, "^trident-vm-.*-testimage.qcow2$")
	if err != nil {
		return fmt.Errorf("failed to find image file: %w", err)
	}
	logrus.Tracef("Found image file: %s", imageFile)

	bootImage := artifactsDir + "/booted.qcow2"
	if err := exec.Command("cp", imageFile, bootImage).Run(); err != nil {
		return fmt.Errorf("failed to copy image: %w", err)
	}
	logrus.Tracef("Copied image to boot image: %s", bootImage)

	err = cfg.createQemuVM(vmName, bootImage, true)
	if err != nil {
		return fmt.Errorf("failed to create VM: %w", err)
	}

	// Wait for serial log
	for {
		if _, err := os.Stat(cfg.SerialLog); err == nil {
			break
		}
	}

	logrus.Tracef("Check if VM is ready for login")
	err = cfg.WaitForLogin(vmName, outputPath, verbose, 0)
	if err != nil {
		return fmt.Errorf("failed to wait for login after reboot: %w", err)
	}

	return nil
}

func (cfg QemuConfig) CleanupQemuVM(vmName string) error {
	err := cfg.deleteLibvirtDomain(vmName)
	if err != nil {
		return fmt.Errorf("failed to cleanup vm '%s': %w", vmName, err)
	}
	return nil
}

func (cfg QemuConfig) RebootQemuVm(vmName string, iteration int, outputPath string, verbose bool) error {
	logrus.Tracef("Truncate log files before reboot")
	if err := cfg.TruncateLog(vmName); err != nil {
		return fmt.Errorf("failed to truncate log file: %w", err)
	}

	lv, domain, err := getLibvirtDomainByname(vmName)
	if err != nil {
		return fmt.Errorf("failed to lookup domain by name '%s': %w", vmName, err)
	}

	logrus.Tracef("Rebooting VM '%s' before update attempt #%d", vmName, iteration)
	if err := lv.DomainShutdown(domain); err != nil {
		return fmt.Errorf("failed to shutdown domain '%s': %w", vmName, err)
	}
	logrus.Tracef("Waiting for VM '%s' to shut down", vmName)
	for {
		domainState, _, err := lv.DomainGetState(domain, 0)
		if err != nil {
			return fmt.Errorf("failed to get domain state: %w", err)
		}
		if domainState == int32(libvirt.DomainShutoff) {
			break // Domain is shut off, exit loop
		}
	}
	logrus.Tracef("Domain '%s' is shut down, starting it again", vmName)
	err = lv.DomainCreate(domain)
	if err != nil {
		return fmt.Errorf("failed to start domain '%s': %w", vmName, err)
	}
	logrus.Tracef("Waiting for VM '%s' to come back up after reboot", vmName)
	err = cfg.WaitForLogin(vmName, outputPath, verbose, iteration)
	if err != nil {
		return fmt.Errorf("failed to wait for login after reboot: %w", err)
	}
	return nil
}

func getLibvirtDomainByname(vmName string) (lv *libvirt.Libvirt, domain libvirt.Domain, err error) {
	uri, _ := url.Parse(string(libvirt.QEMUSession))
	lv, err = libvirt.ConnectToURI(uri)
	if err != nil {
		return lv, domain, fmt.Errorf("failed to connect: %w", err)
	}

	domain, err = lv.DomainLookupByName(vmName)
	if err != nil {
		return lv, domain, fmt.Errorf("failed to lookup domain by name '%s': %w", vmName, err)
	}

	return lv, domain, nil
}

func (cfg QemuConfig) getQemuVmIpAddresses(vmName string) ([]string, error) {
	lv, domain, err := getLibvirtDomainByname(vmName)
	if err != nil {
		return nil, fmt.Errorf("failed to lookup domain by name '%s': %w", vmName, err)
	}
	logrus.Tracef("Found libvirt domain '%s' with ID %v", vmName, domain)

	ifaces, err := lv.DomainInterfaceAddresses(domain, uint32(libvirt.DomainInterfaceAddressesSrcLease), 0) // VIR_DOMAIN_INTERFACE_ADDRESSES_SRC_AGENT retrieves info from the guest agent
	if err != nil {
		return nil, fmt.Errorf("failed to get domain interface addresses: %w", err)
	}
	logrus.Tracef("Found %d interfaces for domain '%s'", len(ifaces), vmName)

	ipAddressesFound := make([]string, 0)

	// Iterate through interfaces to find IP address
	for _, val := range ifaces {
		logrus.Tracef("Interface '%s' has %d addresses", val.Name, len(val.Addrs))
		if val.Addrs != nil {
			logrus.Tracef("Interface '%s' has non-null addresses: %v", val.Name, val.Addrs)
			for _, addr := range val.Addrs {
				logrus.Tracef("Found address '%s' of type %d for interface '%s'", addr.Addr, addr.Type, val.Name)
				if addr.Type == int32(libvirt.IPAddrTypeIpv4) {
					logrus.Tracef("Found IPv4 address '%s' for interface '%s'", addr.Addr, val.Name)
					ipAddressesFound = append(ipAddressesFound, addr.Addr)
				}
			}
		}
	}
	return ipAddressesFound, nil
}

func (cfg QemuConfig) GetAllVmIPAddresses(vmName string) ([]string, error) {
	const maxStabilizeRetries = 5
	for {
		ips, err := cfg.getQemuVmIpAddresses(vmName)
		if err != nil || len(ips) == 0 {
			logrus.Tracef("Failed to get QEMU VM IP addresses: %v", err)

			virshOutput, virshErr := exec.Command("sudo", "virsh", "domifaddr", vmName).CombinedOutput()
			logrus.Tracef("virsh domifaddr output: %s\n%v", string(virshOutput), virshErr)

			time.Sleep(1 * time.Second) // Wait before retrying
			continue                    // Retry until we get an IP address
		}

		// If multiple IPs are found, the DHCP lease table may not have settled yet.
		// This occurs because DomainInterfaceAddresses with SrcLease queries the
		// dnsmasq lease file, which can temporarily contain multiple entries for the
		// same MAC address when a VM sends duplicate DHCPDISCOVER packets at boot
		// (a known race condition with libvirt's default network).
		//
		// See: https://libvirt.org/html/libvirt-libvirt-domain.html#virDomainInterfaceAddresses
		//   "VIR_DOMAIN_INTERFACE_ADDRESSES_SRC_LEASE queries the DHCP leases
		//    maintained by the network's DHCP server" — multiple addresses are
		//    returned when previous leases are still valid or the address has changed.
		//
		// Wait and re-query to see if it stabilizes to a single IP.
		if len(ips) > 1 {
			logrus.Warnf("Found %d IPs for VM '%s', waiting for DHCP lease to stabilize", len(ips), vmName)
			latestIps := ips
			for i := 0; i < maxStabilizeRetries; i++ {
				time.Sleep(2 * time.Second)
				retryIps, retryErr := cfg.getQemuVmIpAddresses(vmName)
				if retryErr != nil || len(retryIps) == 0 {
					continue
				}
				latestIps = retryIps
				if len(retryIps) == 1 {
					logrus.Infof("DHCP lease stabilized to single IP '%s' after %d retries", retryIps[0], i+1)
					return retryIps, nil
				}
			}
			logrus.Warnf("DHCP lease did not stabilize after %d retries, proceeding with first IP '%s' from %v",
				maxStabilizeRetries, latestIps[0], latestIps)
			return latestIps[:1], nil
		}

		return ips, nil
	}
}

func (cfg QemuConfig) deleteLibvirtDomain(vmName string) error {
	logrus.Tracef("Deleting libvirt domain '%s'", vmName)
	lv, domain, err := getLibvirtDomainByname(vmName)
	if err != nil {
		logrus.Tracef("Failed to lookup domain by name '%s': %v", vmName, err)
		return nil
	}

	domainState, _, err := lv.DomainGetState(domain, 0)
	if err != nil {
		return fmt.Errorf("failed to get domain state: %w", err)
	}
	if domainState == int32(libvirt.DomainRunning) {
		logrus.Tracef("Destroying libvirt domain '%s'", vmName)
		err = lv.DomainDestroy(domain) // Stop the VM
		if err != nil {
			logrus.Tracef("failed to destroy domain '%s': %v", vmName, err)
			return fmt.Errorf("failed to destroy domain '%s': %w", vmName, err)
		}
	}

	logrus.Tracef("Undefining libvirt domain '%s'", vmName)
	err = lv.DomainUndefineFlags(domain, libvirt.DomainUndefineNvram) // Undefine the VM, including NVRAM
	if err != nil {
		logrus.Tracef("failed to undefine domain '%s': %v", vmName, err)
		return fmt.Errorf("failed to undefine domain '%s': %w", vmName, err)
	}
	return nil
}

func (cfg QemuConfig) createQemuVM(name string, bootImage string, useVirtInstall bool) error {

	// TODO: migrate to use virtdeploy

	if useVirtInstall {
		logrus.Tracef("Using virt-install to create QEMU VM '%s'", name)
		virtInstallArgs := []string{
			"virt-install",
			"--name", name,
			"--memory", "2048",
			"--vcpus", "2",
			"--os-variant", "generic",
			"--import",
			"--disk", fmt.Sprintf("%s,bus=sata", bootImage),
			"--network", "default",
			"--noautoconsole",
			"--serial", fmt.Sprintf("file,path=%s", cfg.SerialLog),
		}
		if cfg.SecureBoot {
			virtInstallArgs = append(virtInstallArgs, "--machine", "q35", "--boot", "uefi,loader_secure=yes")
		} else {
			virtInstallArgs = append(virtInstallArgs, "--boot", "uefi,loader_secure=no")
		}
		logrus.Tracef("Running virt-install command: %s", strings.Join(virtInstallArgs, " "))
		if err := exec.Command("sudo", virtInstallArgs...).Run(); err != nil {
			return fmt.Errorf("failed to create QEMU VM '%s': %w", name, err)
		}
	} else {
		logrus.Tracef("Using libvirt to create QEMU VM '%s'", name)
		uri, _ := url.Parse(string(libvirt.QEMUSession))
		lv, err := libvirt.ConnectToURI(uri)
		if err != nil {
			return fmt.Errorf("failed to connect: %w", err)
		}

		loaderPath := "/usr/share/OVMF/OVMF_CODE.secboot.fd"
		if !cfg.SecureBoot {
			loaderPath = "/usr/share/OVMF/OVMF_CODE.fd"
		}

		domainXML := fmt.Sprintf(`
<domain type='kvm'>
	<name>%s</name>
	<memory unit='MiB'>2048</memory>
	<vcpu>2</vcpu>
	<os>
	<type arch='x86_64' machine='q35'>hvm</type>
	<loader readonly='yes' type='pflash'>%s</loader>
	<boot dev='hd'/>
	</os>
	<features>
	<acpi/>
	<apic/>
	<pae/>
	</features>
	<cpu mode='host-model'/>
	<devices>
	<disk type='file' device='disk'>
		<driver name='qemu' type='qcow2'/>
		<source file='%s'/>
		<target dev='sda' bus='sata'/>
	</disk>
	<interface type='network'>
		<source network='default'/>
		<model type='virtio'/>
	</interface>
	<serial type='file'>
		<source path='%s'/>
	</serial>
	<console type='file'>
		<source path='%s'/>
	</console>
	</devices>
</domain>`,
			name, loaderPath, bootImage, cfg.SerialLog, cfg.SerialLog)

		logrus.Tracef("Defining libvirt domain with XML: %s", domainXML)
		domain, err := lv.DomainDefineXML(domainXML)
		if err != nil {
			return fmt.Errorf("failed to define domain: %w", err)
		}

		logrus.Tracef("Starting libvirt domain '%s'", name)
		if err := lv.DomainCreate(domain); err != nil {
			return fmt.Errorf("failed to start domain: %w", err)
		}
	}
	return nil
}

func (cfg QemuConfig) TruncateLog(vmName string) error {
	// If domain exists and serial log file exists, truncate the serial log file
	if _, _, err := getLibvirtDomainByname(vmName); err == nil {
		if _, err := os.Stat(cfg.SerialLog); err == nil {
			if err := exec.Command("truncate", "-s", "0", cfg.SerialLog).Run(); err != nil {
				return fmt.Errorf("failed to truncate log file: %w", err)
			}
		}
	}
	return nil
}

func (cfg QemuConfig) WaitForLogin(vmName string, outputPath string, verbose bool, iteration int) error {
	localSerialLog := "./serial.log"
	// Wait for login prompt to appear in the serial log and save the log to localSerialLog
	waitErr := innerWaitForLogin(cfg.SerialLog, verbose, iteration, localSerialLog)
	// Copy serial log to output directory if specified
	if outputPath != "" {
		err := os.MkdirAll(outputPath, 0755)
		if err != nil {
			return fmt.Errorf("failed to create output directory '%s': %w", outputPath, err)
		}

		outputFilename := fmt.Sprintf("%s-serial.log", fmt.Sprintf("%03d", iteration))
		if err := exec.Command("cp", localSerialLog, filepath.Join(outputPath, outputFilename)).Run(); err != nil {
			return fmt.Errorf("failed to copy serial log to output directory: %w", err)
		}
	}

	// Truncate the serial log after saving to prevent unbounded growth across iterations.
	// Without truncation, the serial log accumulates all prior boot sequences, eventually
	// causing the serial console capture to be cut off mid-boot.
	if truncErr := cfg.TruncateLog(vmName); truncErr != nil {
		logrus.Warnf("Failed to truncate serial log after iteration %d: %v", iteration, truncErr)
	}

	if waitErr != nil {
		// Serial login detection failed. Before declaring the VM dead, check if it
		// actually booted and acquired a DHCP lease.
		//
		// Background: serial-getty@ttyS0.service depends on dev-ttyS0.device, which
		// is auto-generated by systemd when udev reports the device. The service also
		// has ConditionPathExists=/dev/ttyS0. If udev is slightly slow creating the
		// device node, systemd evaluates the condition before the node exists and
		// skips the device unit entirely — so serial-getty never starts and no
		// "login:" prompt appears on the serial console.
		//
		// This is a known systemd race condition that affects ~2% of boots,
		// regardless of host load:
		//   https://github.com/systemd/systemd/issues/10850
		//
		// The QEMU serial backend (file vs pty) does not affect this — the guest
		// sees the same 16550A UART either way. The race is purely in the timing
		// of udev device node creation vs systemd condition evaluation.
		//
		// Fallback strategy: query the libvirt DHCP lease table. If the VM has
		// acquired an IP address, it booted far enough for networking to start,
		// confirming the VM is alive despite no serial login prompt.
		logrus.Warnf("Serial login detection failed for iteration %d, checking DHCP lease as fallback: %v", iteration, waitErr)

		// Try a few times to get the DHCP lease — the VM may still be acquiring one.
		var ips []string
		for attempt := 0; attempt < 10; attempt++ {
			ips, _ = cfg.getQemuVmIpAddresses(vmName)
			if len(ips) > 0 {
				break
			}
			time.Sleep(3 * time.Second)
		}

		if len(ips) > 0 {
			logrus.Warnf("VM '%s' has DHCP lease (IP: %s) despite no serial login — VM likely booted but serial-getty did not start (ttyS0 device skipped by systemd)", vmName, ips[0])
			// VM has an IP — it booted far enough for networking. Return success
			// so the test can proceed with SSH-based operations.
			return nil
		}

		// VM has no DHCP lease — it's genuinely stuck
		logrus.Errorf("Failed to reach login prompt for the VM for iteration %d: %v", iteration, waitErr)
		if err := analyzeSerialLog(cfg.SerialLog); err != nil {
			return err
		}

		dominfoOut, err := exec.Command("virsh", "dominfo", vmName).Output()
		if err != nil {
			logrus.Errorf("Failed to get domain info for VM '%s': %v", vmName, err)
		} else {
			logrus.Infof("Domain info for VM '%s': %s", vmName, dominfoOut)
		}

		dfOut, err := exec.Command("df", "-h").Output()
		if err != nil {
			logrus.Errorf("Failed to run 'df -h': %v", err)
		} else {
			logrus.Infof("Disk usage:\n%s", dfOut)
		}
	}
	return waitErr
}

func analyzeSerialLog(serial string) error {
	lastLines, err := exec.Command("tail", "-n", "100", serial).Output()
	if err != nil {
		logrus.Warnf("Failed to read serial log tail: %v", err)
		return nil
	}
	serialTail := string(lastLines)

	// Always print the serial log tail on failure to aid diagnosis.
	// Without this output, boot failures are nearly impossible to root-cause
	// because the serial log is only available inside large pipeline artifacts.
	logrus.Infof("Serial log tail (last 100 lines):\n%s", serialTail)

	// Check for known boot failure patterns and return a specific error
	// if one is found, so the failure category is clear in pipeline results.

	if strings.Contains(serialTail, "tpm tpm0: Operation Timed out") {
		return fmt.Errorf("tpm tpm0: Operation Timed out")
	}

	// Dracut/initramfs emergency shell — VM booted but initramfs could not
	// mount the root filesystem or a required module was missing.
	// See ADO#10589 for an example caused by dracut temp-dir name collision.
	if strings.Contains(serialTail, "Entering emergency mode") ||
		strings.Contains(serialTail, "dracut-emergency") ||
		strings.Contains(serialTail, "Cannot open shared object file") {
		return fmt.Errorf("VM stuck in initramfs emergency shell (see serial log above)")
	}

	// Dracut-initqueue timeout — initramfs is waiting for a device that doesn't
	// exist, typically caused by stale disk UUIDs embedded in initramfs (bug 15086).
	// The serial log shows dracut-initqueue repeatedly trying to find the device.
	if strings.Contains(serialTail, "dracut-initqueue") ||
		strings.Contains(serialTail, "Timed out waiting for device") ||
		strings.Contains(serialTail, "Could not boot") {
		return fmt.Errorf("VM stuck in initramfs waiting for device — likely stale UUID in initramfs (bug 15086, see serial log above)")
	}

	// Kernel panic — the kernel itself crashed during boot.
	if strings.Contains(serialTail, "Kernel panic") ||
		strings.Contains(serialTail, "end Kernel panic") {
		return fmt.Errorf("kernel panic during boot (see serial log above)")
	}

	// GRUB error — bootloader could not find the kernel or boot entry.
	if strings.Contains(serialTail, "error: no such device") ||
		strings.Contains(serialTail, "error: file") {
		return fmt.Errorf("GRUB boot error (see serial log above)")
	}

	return nil
}

func innerWaitForLogin(vmSerialLog string, verbose bool, iteration int, localSerialLog string) error {
	// 180 seconds gives headroom for slow boots under resource pressure at scale.
	// A normal Azure Linux boot takes 10-30 seconds; the extra margin accounts for
	// host CPU contention when many QEMU VMs run on the same agent.
	return stormutils.WaitForLoginMessageInSerialLog(vmSerialLog, verbose, iteration, localSerialLog, time.Second*180)
}
