package virtdeploy

import "github.com/google/uuid"

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
	IPAddress string `json:"ipAddress"`

	// MAC address assigned to the VM
	MACAddress string `json:"macAddress"`

	// UUID of the libvirt domain
	Uuid uuid.UUID `json:"uuid"`

	// Path to the VM's NVRAM file on the host
	NvramPath string `json:"nvramPath"`
}
