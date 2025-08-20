// Package storm provides helpers for Trident loop-update Storm tests.
// This file contains helpers converted from Bash scripts in scripts/loop-update.
package qemu

import (
	"bufio"
	"fmt"
	"io"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"storm/pkg/storm/utils"
	"strings"
	"time"
	"tridenttools/storm/servicing/utils/file"

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
	imageFile, err := file.FindFile(artifactsDir, "^trident-vm-.*-testimage.qcow2$")
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
		return lv, domain, fmt.Errorf("failed to connect: %v", err)
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
	for {
		ips, err := cfg.getQemuVmIpAddresses(vmName)
		if err != nil || len(ips) == 0 {
			logrus.Tracef("Failed to get QEMU VM IP addresses: %v", err)

			virshOutput, virshErr := exec.Command("sudo", "virsh", "domifaddr", vmName).CombinedOutput()
			logrus.Tracef("virsh domifaddr output: %s\n%v", string(virshOutput), virshErr)

			time.Sleep(1 * time.Second) // Wait before retrying
			continue                    // Retry until we get an IP address
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
			return fmt.Errorf("failed to connect: %v", err)
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
	// If domain exists, truncate the serial log file
	if _, _, err := getLibvirtDomainByname(vmName); err == nil {
		if err := exec.Command("truncate", "-s", "0", cfg.SerialLog).Run(); err != nil {
			return fmt.Errorf("failed to truncate log file: %w", err)
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

	if waitErr != nil {
		// Create fairly generic error message
		logrus.Errorf("Failed to reach login prompt for the VM for iteration %d: %v", iteration, waitErr)
		// Attempt to create more meaningful error messages based on the serial log
		if err := analyzeSerialLog(cfg.SerialLog); err != nil {
			return err
		}

		// Output qemu domain info to try to help debug failure
		dominfoOut, err := exec.Command("virsh", "dominfo", vmName).Output()
		if err != nil {
			logrus.Errorf("Failed to get domain info for VM '%s': %v", vmName, err)
		} else {
			logrus.Infof("Domain info for VM '%s': %s", vmName, dominfoOut)
		}

		// Output disk usage to help debug failure
		dfOut, err := exec.Command("df", "-h").Output()
		if err != nil {
			logrus.Errorf("Failed to run 'df -h': %v", err)
		} else {
			logrus.Infof("Disk usage:\n%s", dfOut)
		}
	}
	return waitErr
}

func printAndSave(line string, verbose bool, localSerialLog string) {
	if line == "" {
		return
	}

	// Remove ANSI control codes
	line = utils.ANSI_CONTROL_CLEANER.ReplaceAllString(line, "")
	if verbose {
		logrus.Info(line)
	}
	if localSerialLog != "" {
		// Remove all ANSI escape codes
		line = utils.ANSI_CLEANER.ReplaceAllString(line, "")
		logFile, err := os.OpenFile(localSerialLog, os.O_APPEND|os.O_CREATE|os.O_RDWR, 0644)
		if err != nil {
			return
		}
		defer logFile.Close()

		_, err = logFile.WriteString(line + "\n")
		if err != nil {
			logrus.Errorf("Failed to append line to output file: %v", err)
		}
	}
}

func analyzeSerialLog(serial string) error {
	// Read the last line of the serial log
	lastLines, err := exec.Command("tail", "-n", "100", serial).Output()
	// Watch for specific failures and create error messages accordingly
	if err == nil && strings.Contains(string(lastLines), "tpm tpm0: Operation Timed out") {
		return fmt.Errorf("tpm tpm0: Operation Timed out")
	}
	return nil
}

func innerWaitForLogin(vmSerialLog string, verbose bool, iteration int, localSerialLog string) error {
	// Timeout for monitoring serial log for login prompt
	timeout := time.Second * 120
	startTime := time.Now()

	// Wait for serial log
	for {
		if time.Since(startTime) >= timeout {
			return fmt.Errorf("timeout waiting for serial log after %d seconds", int(timeout.Seconds()))
		}
		if _, err := os.Stat(vmSerialLog); err == nil {
			break
		}
	}

	// Create the file if it doesn't exist
	file, err := os.OpenFile(vmSerialLog, os.O_RDWR, 0644)
	if err != nil {
		return fmt.Errorf("failed to open serial log file: %w", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file)
	lineBuffer := ""
	for {
		// Check if the current line contains the login prompt, and return if it does
		if strings.Contains(lineBuffer, "login:") && !strings.Contains(lineBuffer, "mos") {
			printAndSave(lineBuffer, verbose, localSerialLog)
			return nil
		}

		// Read a rune from reader, if EOF is encountered, retry until either a new
		// character is read or the timeout is reached
		var readRune rune
		for {
			if time.Since(startTime) >= timeout {
				return fmt.Errorf("timeout waiting for login prompt after %d seconds", int(timeout.Seconds()))
			}
			// Read a rune from the serial log file
			readRune, _, err = reader.ReadRune()
			if err == io.EOF {
				// Wait for new serial output
				time.Sleep(10 * time.Millisecond)
				continue
			}
			if err != nil {
				return fmt.Errorf("failed to read from serial log: %w", err)
			}
			// Successfully read a rune, break out of the loop
			break
		}
		// Handle the rune read from the serial log
		runeStr := string(readRune)
		if runeStr == "\n" {
			// If the last character is a newline, print the line buffer
			// and reset it
			printAndSave(lineBuffer, verbose, localSerialLog)
			lineBuffer = ""
		} else {
			// If non-newline, append the output to the buffer
			lineBuffer += runeStr
		}
	}
}
