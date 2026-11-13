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

func (n namespace) isInNamespace(name string) bool {
	if name == n.libvirtNetworkName() || name == n.storagePoolName() || name == n.nvramPoolName() {
		return true
	}
	index, found := strings.CutPrefix(name, string(n)+"-vm-")
	if !found {
		return false
	}
	vmIndex, err := strconv.Atoi(index)
	return err == nil && vmIndex >= 0 && name == n.vmName(vmIndex)
}
