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

func (n namespace) vmName(index int) string {
	return string(n) + "-vm-" + fmt.Sprintf("%d", index)
}

func (n namespace) matchesDomainName(name string) bool {
	index, found := strings.CutPrefix(name, string(n)+"-vm-")
	if !found {
		return false
	}
	vmIndex, err := strconv.Atoi(index)
	return err == nil && vmIndex >= 0 && name == n.vmName(vmIndex)
}

func (n namespace) poolTarget(name string) (string, bool) {
	switch name {
	case n.storagePoolName():
		return "/var/lib/libvirt/" + string(n), true
	case n.nvramPoolName():
		return "/var/lib/" + string(n) + "-nvram", true
	default:
		return "", false
	}
}
