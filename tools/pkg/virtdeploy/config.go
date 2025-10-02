package virtdeploy

import (
	"fmt"
	"net"
	"os"

	"github.com/digitalocean/go-libvirt"
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

	// UUID of the VM according to libvirt
	domain libvirt.Domain

	// Storage volumes for the VM
	volumes []storageVolume

	// CDROM drives for the VM
	cdroms []cdrom

	// Firmware template path
	firmwareLoaderPath string

	// NVRAM file and path
	nvramFile string
	nvramPath string

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

	return nil
}

type CloudInitConfig struct {
	Userdata string
	Metadata string
}
