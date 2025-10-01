package virtdeploy

import (
	"fmt"
	"strconv"

	"github.com/digitalocean/go-libvirt"
	libvirtxml "libvirt.org/libvirt-go-xml"
)

type storagePool struct {
	name   string
	path   string
	owner  int
	group  int
	mode   string
	lvPool libvirt.StoragePool
}

func newPool(name string, path string) storagePool {
	return storagePool{
		name:  name,
		path:  path,
		owner: -1,
		group: -1,
		mode:  "0755",
	}
}

func (n storagePool) asXml() (string, error) {
	pool := libvirtxml.StoragePool{
		Name: n.name,
		Type: "dir",
		Target: &libvirtxml.StoragePoolTarget{
			Path: n.path,
			Permissions: &libvirtxml.StoragePoolTargetPermissions{
				Owner: strconv.Itoa(n.owner),
				Group: strconv.Itoa(n.group),
				Mode:  n.mode,
			},
		},
	}

	xml, err := pool.Marshal()
	if err != nil {
		return "", fmt.Errorf("failed to marshal storage pool XML: %w", err)
	}

	return xml, nil
}
