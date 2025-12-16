package virtdeploy

import (
	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	"libvirt.org/go/libvirtxml"
)

type VirtDeployStatus struct {
	// Namespace in which the resources were created
	Namespace string `json:"namespace"`

	// CIDR of the virtual network
	NetworkCIDR string `json:"networkCIDR"`

	// Status of the created VMs
	VMs []VirtDeployVMStatus `json:"vms"`
}

type VirtDeployVMStatus struct {
	// Name of the VM
	Name string `json:"name"`

	// IP address assigned to the VM
	IPAddress string `json:"ip"`

	// MAC address assigned to the VM
	MACAddress string `json:"macAddress"`

	// UUID of the libvirt domain
	Uuid uuid.UUID `json:"uuid"`

	// Path to the VM's NVRAM file on the host
	NvramPath string `json:"nvramPath"`

	// Full XML definition of the libvirt domain at creation time.
	Definition *libvirtxml.Domain `json:"-"`
}

func (v *VirtDeployVMStatus) LibvirtUUID() libvirt.UUID {
	return libvirt.UUID(v.Uuid)
}
