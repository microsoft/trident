package virtdeploy

import (
	"fmt"
	"net"
	"os"

	"github.com/digitalocean/go-libvirt"
	"libvirt.org/go/libvirtxml"
)

type VirtDeployConfig struct {
	// Namespace to create resources in
	Namespace string

	// Virtual network configuration
	IPNet net.IPNet

	// Interface on the host to give the VM outbound network access.
	// Set to "auto" to automatically detect the default interface.
	// Set to "" to disable outbound network access.
	NatInterface string

	// List of VMs to create
	VMs []VirtDeployVM

	// Start the VMs after creation
	StartVMs bool
}

func (c VirtDeployConfig) validate() error {
	if c.Namespace == "" {
		return fmt.Errorf("namespace must be specified")
	}

	if c.IPNet.IP == nil || c.IPNet.IP.Equal(net.IPv4zero) {
		return fmt.Errorf("a valid network IP must be specified")
	}

	for i := range c.VMs {
		if err := c.VMs[i].validate(); err != nil {
			return fmt.Errorf("VM %d: validation failed: %w", i, err)
		}
	}

	return nil
}

type VirtDeployVM struct {
	// Internal fields

	// Name of the VM
	name string

	// Leased IP address for the VM
	ipAddr net.IP

	// MAC address of the VM
	mac macAddress

	// Domain metadata from libvirt
	domain libvirt.Domain

	// Storage volumes for the VM
	volumes []storageVolume

	// CDROM drives for the VM
	cdroms []cdrom

	// Firmware loader path
	firmwareLoaderPath string

	// Firmware vars template path
	firmwareVarsTemplatePath string

	// NVRAM file and path
	nvramFile string
	nvramPath string

	// Ignition config file volume path
	ignitionVolume string

	// Network name to attach the VM to
	networkName string

	// Final domain definition at creation time
	domainDefinition *libvirtxml.Domain

	// User-configurable fields

	// Number of virtual CPUs
	Cpus uint

	// Amount of memory (in GiB)
	Mem uint

	// List of disk sizes (in GiB)
	Disks []uint

	// Optional path to the OS disk to attach to the first disk. If empty, a
	// blank disk will be created.
	OsDiskPath string

	// Optional cloud-init configuration
	CloudInit *CloudInitConfig

	// Configure secure boot firmware
	SecureBoot bool

	// Configure an emulated TPM device
	EmulatedTPM bool

	// Architecture of the VM (amd64 or arm64)
	Arch string

	// Ignition config file to pass to the VM (for ACL)
	IgnitionConfigPath string
}

func (vm VirtDeployVM) validate() error {
	if vm.Cpus == 0 {
		return fmt.Errorf("CPU count must be > 0")
	}
	if vm.Mem == 0 {
		return fmt.Errorf("memory must be > 0")
	}
	if len(vm.Disks) == 0 {
		return fmt.Errorf("at least one disk must be specified")
	}
	for j, sz := range vm.Disks {
		if sz == 0 {
			return fmt.Errorf("disk %d size must be > 0", j)
		}
	}

	if vm.OsDiskPath != "" {
		if _, err := os.Stat(vm.OsDiskPath); err != nil {
			return fmt.Errorf("OS disk path is invalid: %w", err)
		}
	}

	if vm.CloudInit != nil {
		if vm.CloudInit.Userdata == "" {
			return fmt.Errorf("cloud-init user file path must be specified if cloud-init config is provided")
		}
		if vm.CloudInit.Metadata == "" {
			return fmt.Errorf("cloud-init metadata file path must be specified if cloud-init config is provided")
		}
		if _, err := os.Stat(vm.CloudInit.Userdata); err != nil {
			return fmt.Errorf("cloud-init user file path is invalid: %w", err)
		}
		if _, err := os.Stat(vm.CloudInit.Metadata); err != nil {
			return fmt.Errorf("cloud-init metadata file path is invalid: %w", err)
		}
	}

	if vm.Arch != "amd64" && vm.Arch != "arm64" {
		return fmt.Errorf("unsupported architecture '%s'", vm.Arch)
	}

	if vm.IgnitionConfigPath != "" {
		if _, err := os.Stat(vm.IgnitionConfigPath); err != nil {
			return fmt.Errorf("ignition config path is invalid: %w", err)
		}
	}

	return nil
}

type CloudInitConfig struct {
	Userdata string
	Metadata string
}
