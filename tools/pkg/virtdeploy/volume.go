package virtdeploy

import (
	"fmt"

	"github.com/digitalocean/go-libvirt"
	"libvirt.org/go/libvirtxml"
)

type storageVolume struct {
	// Name of the volume according to libvirt
	name string
	// Device path for the volume inside the VM
	device string
	// Filesystem path for the volume on the host
	path string
	// Size of the volume in GB
	size uint
	// Optional OS disk image to upload to the volume
	osDisk string
	// Libvirt storage volume, to be populated when created
	lvVol libvirt.StorageVol
}

func newSimpleVolume(name string, size uint) storageVolume {
	return storageVolume{
		name: name,
		size: size,
	}
}

func (n storageVolume) asXml() (string, error) {
	vol := libvirtxml.StorageVolume{
		Name: n.name,
		Capacity: &libvirtxml.StorageVolumeSize{
			Unit:  "G",
			Value: uint64(n.size),
		},
		Target: &libvirtxml.StorageVolumeTarget{
			Path: n.path,
			Format: &libvirtxml.StorageVolumeTargetFormat{
				Type: "qcow2",
			},
			Permissions: &libvirtxml.StorageVolumeTargetPermissions{
				Mode: "0744",
			},
		},
	}

	xml, err := vol.Marshal()
	if err != nil {
		return "", fmt.Errorf("failed to marshal storage volume XML: %w", err)
	}

	return xml, nil
}
