package virtdeploy

import (
	"fmt"
	"strconv"
	"strings"
)

type namespace string

func (n namespace) String() string {
	return string(n)
}

func (n namespace) libvirtNetworkName() string {
	return string(n) + "-network"
}

func (n namespace) storagePoolName() string {
	return string(n) + "-pool"
}

func (n namespace) nvramPoolName() string {
	return string(n) + "-nvram-pool"
}

func (n namespace) storagePoolPath() string {
	return fmt.Sprintf("/var/lib/libvirt/%s", n)
}

func (n namespace) nvramPoolPath() string {
	return fmt.Sprintf("/var/lib/%s-nvram", n)
}

func (n namespace) vmName(index int) string {
	return string(n) + "-vm-" + fmt.Sprintf("%d", index)
}

func (n namespace) isInNamespace(name string) bool {
	if name == n.libvirtNetworkName() || name == n.storagePoolName() || name == n.nvramPoolName() {
		return true
	}
	return n.ownsDomainName(name)
}

func (n namespace) ownsDomainName(name string) bool {
	index, found := strings.CutPrefix(name, string(n)+"-vm-")
	if !found {
		return false
	}
	vmIndex, err := strconv.Atoi(index)
	return err == nil && vmIndex >= 0 && name == n.vmName(vmIndex)
}
