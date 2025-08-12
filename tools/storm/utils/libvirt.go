package utils

import (
	"fmt"
	"net/url"
	"os"
	"os/exec"

	libvirtxml "libvirt.org/libvirt-go-xml"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	"github.com/sirupsen/logrus"
)

type LibvirtVm struct {
	libvirt *libvirt.Libvirt
	domain  libvirt.Domain
}

func InitializeVm(vmUuid uuid.UUID) (*LibvirtVm, error) {
	logrus.Infof("Initializing VM with UUID '%s'", vmUuid.String())

	uri, _ := url.Parse(string(libvirt.QEMUSession))
	l, err := libvirt.ConnectToURI(uri)
	if err != nil {
		return nil, fmt.Errorf("failed to connect: %v", err)
	}

	var uuidSlice [16]byte
	copy(uuidSlice[:], vmUuid[:])

	domain, err := l.DomainLookupByUUID(uuidSlice)
	if err != nil {
		return nil, fmt.Errorf("failed to lookup domain by UUID '%s': %w", vmUuid.String(), err)
	}

	domainState, _, err := l.DomainGetState(domain, 0)
	if err != nil {
		return nil, fmt.Errorf("failed to get domain state: %w", err)
	}

	// Shutdown the VM if necessary
	if domainState != int32(libvirt.DomainShutoff) {
		logrus.Infof("Shutting down VM '%s'", domain.Name)
		if err = l.DomainDestroy(domain); err != nil {
			logrus.Warnf("failed to reset domain '%s': %s", domain.Name, err.Error())
		}
	}

	return &LibvirtVm{l, domain}, nil
}

func (vm *LibvirtVm) SetFirmwareVars(boot_url string, secure_boot bool) error {
	// Get the domain XML
	domainXml, err := vm.libvirt.DomainGetXMLDesc(vm.domain, libvirt.DomainXMLUpdateCPU)
	if err != nil {
		return fmt.Errorf("failed to get XML description of domain '%s': %w", vm.domain.Name, err)
	}
	logrus.Tracef("Domain XML:\n%s", domainXml)

	// Parse the domain XML
	parsedDomainXml := &libvirtxml.Domain{}
	if err := parsedDomainXml.Unmarshal(domainXml); err != nil {
		return fmt.Errorf("failed to parse domain XML: %w", err)
	}

	// Find the NVRAM path
	nvram := parsedDomainXml.OS.NVRam
	if nvram != nil {
		logrus.Debugf("Extracted NVRAM path: %s", nvram.NVRam)
	} else {
		return fmt.Errorf("no <nvram> node found in domain XML")
	}

	// Check if a file exists at the NVRAM path
	if _, err := os.Stat(nvram.NVRam); err != nil {
		// If not, start the VM in a paused state and then immediately stop it.
		// This will cause libvirt to create the NVRAM file.
		if vm.domain, err = vm.libvirt.DomainCreateWithFlags(vm.domain, uint32(libvirt.DomainStartPaused)); err != nil {
			return fmt.Errorf("failed to create domain '%s': %w", vm.domain.Name, err)
		}
		if err = vm.libvirt.DomainDestroy(vm.domain); err != nil {
			return fmt.Errorf("failed to destroy domain '%s': %w", vm.domain.Name, err)
		}
	}

	args := []string{"--inplace", nvram.NVRam, "--set-boot-uri", boot_url}
	if secure_boot {
		args = append(args, "--set-true", "SecureBootEnable")
	} else {
		args = append(args, "--set-false", "SecureBootEnable")
	}

	cmd := exec.Command("virt-fw-vars", args...)
	if output, err := cmd.CombinedOutput(); err != nil {
		logrus.Debugf("virt-fw-vars output:\n%s\n", output)
		return fmt.Errorf("failed to set boot URI: %w", err)
	}
	logrus.Infof("Set boot URI to %s and set SecureBoot to %t", boot_url, secure_boot)

	return nil
}

func (vm *LibvirtVm) Start() error {
	logrus.Infof("Starting VM '%s'", vm.domain.Name)

	if err := vm.libvirt.DomainCreate(vm.domain); err != nil {
		logrus.Errorf("failed to start domain '%s'", vm.domain.Name)
		return err
	}

	return nil
}

func (vm *LibvirtVm) Disconnect() {
	if err := vm.libvirt.Disconnect(); err != nil {
		logrus.Errorf("failed to disconnect from libvirt: %s", err.Error())
	}
}
