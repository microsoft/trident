package utils

import (
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"

	libvirtxml "libvirt.org/libvirt-go-xml"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	"github.com/sirupsen/logrus"
)

const EFI_GLOBAL_VARIABLE_GUID = "8BE4DF61-93CA-11d2-AA0D00E098032B8C"

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

func (vm *LibvirtVm) SetFirmwareVars(bootUrl string, secureBoot bool, signingCert string) error {
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

	// Destroy old instance of VM and its NVRAM file.
	// NVRAM path should stay the same.
	if err := vm.libvirt.DomainUndefineFlags(vm.domain, libvirt.DomainUndefineNvram); err != nil {
		return fmt.Errorf("failed to remove existing NVRAM file: %w", err)
	}
	// Create a new instance of the VM based on domainXml.
	if vm.domain, err = vm.libvirt.DomainDefineXML(domainXml); err != nil {
		return fmt.Errorf("failed to define domain with XML '%s': %w", domainXml, err)
	}
	// Start the VM in a paused state and then immediately stop it.
	// This will cause libvirt to create the NVRAM file.
	if vm.domain, err = vm.libvirt.DomainCreateWithFlags(vm.domain, uint32(libvirt.DomainStartPaused)); err != nil {
		return fmt.Errorf("failed to create domain '%s': %w", vm.domain.Name, err)
	}
	if err = vm.libvirt.DomainDestroy(vm.domain); err != nil {
		return fmt.Errorf("failed to destroy domain '%s': %w", vm.domain.Name, err)
	}

	virtFwVarsArgs := []string{"virt-fw-vars", "--inplace", nvram.NVRam, "--set-boot-uri", bootUrl}

	// Enable SecureBoot, if needed
	if secureBoot {
		logrus.Infof("Setting SecureBoot to enabled")
		virtFwVarsArgs = append(virtFwVarsArgs, "--set-true", "SecureBootEnable")
	} else {
		virtFwVarsArgs = append(virtFwVarsArgs, "--set-false", "SecureBootEnable")
	}

	// Enroll the signing certificate
	if signingCert != "" {
		logrus.Infof("Enrolling signing certificate from %s", signingCert)
		virtFwVarsArgs = append(virtFwVarsArgs, "--enroll-cert", signingCert)
		virtFwVarsArgs = append(virtFwVarsArgs, "--add-db", EFI_GLOBAL_VARIABLE_GUID, signingCert)
	}

	cmd := exec.Command("sudo", virtFwVarsArgs...)
	if output, err := cmd.CombinedOutput(); err != nil {
		logrus.Debugf("virt-fw-vars output:\n%s\n", output)
		return fmt.Errorf("failed to set boot URI: %w", err)
	}
	logrus.Infof("Set boot URI to %s and set SecureBoot to %t", bootUrl, secureBoot)

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

// CaptureScreenshot captures a screenshot of the specified VM and saves it as a PNG file.
// It creates a temporary PPM file, captures the screenshot using virsh, converts it to PNG
// using ImageMagick, and saves it to the specified artifacts folder.
//
// Parameters:
//   - vmName: Name of the VM to capture
//   - artifactsFolder: Directory where the screenshot will be saved
//   - screenshotFilename: Name of the PNG file to create
//
// Returns an error if screenshot capture, conversion, or file operations fail.
func CaptureScreenshot(vmName string, artifactsFolder string, screenshotFilename string) error {
	ppmFilename, err := os.CreateTemp("", "ppm")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	ppmFilename.Close()
	defer os.Remove(ppmFilename.Name())

	err = capturePpmScreenshot(vmName, ppmFilename.Name())
	if err != nil {
		return err
	}

	err = os.MkdirAll(artifactsFolder, 0755)
	if err != nil {
		return err
	}

	pngPath := filepath.Join(artifactsFolder, screenshotFilename)
	if err := convertPpmToPng(ppmFilename.Name(), pngPath); err != nil {
		return err
	}
	return nil
}

func capturePpmScreenshot(vmName string, ppmFilename string) error {
	virshOutput, virshErr := exec.Command("sudo", "virsh", "screenshot", vmName, ppmFilename).CombinedOutput()
	logrus.Tracef("virsh screenshot output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		return virshErr
	}
	return nil
}

func convertPpmToPng(ppmPath string, pngPath string) error {
	virshOutput, virshErr := exec.Command("convert", ppmPath, pngPath).CombinedOutput()
	logrus.Tracef("convert output: %s\n%v", string(virshOutput), virshErr)
	return virshErr
}
