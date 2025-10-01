package virtdeploy

import "fmt"

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
