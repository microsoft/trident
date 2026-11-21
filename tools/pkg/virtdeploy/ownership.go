package virtdeploy

import (
	"encoding/xml"
	"errors"
	"fmt"
	"slices"

	"libvirt.org/go/libvirtxml"
)

var errResourceNotOwned = errors.New("resource ownership could not be verified")

type resourceOwner struct {
	XMLName   xml.Name `xml:"urn:trident:virtdeploy:1 owner"`
	Namespace string   `xml:"namespace,attr"`
	Kind      string   `xml:"kind,attr"`
}

func (n namespace) ownershipMetadata(kind string) (string, error) {
	if n == "" {
		return "", nil
	}
	data, err := xml.Marshal(resourceOwner{Namespace: n.String(), Kind: kind})
	return string(data), err
}

func (n namespace) metadataOwnership(definition, kind string) (present, owned bool, err error) {
	var decoded struct {
		Metadata struct {
			Owners []resourceOwner `xml:"urn:trident:virtdeploy:1 owner"`
		} `xml:"metadata"`
	}
	if err := xml.Unmarshal([]byte(definition), &decoded); err != nil {
		return false, false, err
	}
	owners := decoded.Metadata.Owners
	if len(owners) == 0 {
		return false, false, nil
	}
	return true, len(owners) == 1 &&
		owners[0].Namespace == n.String() && owners[0].Kind == kind, nil
}

func domainNVRAMPath(dom *libvirtxml.Domain) string {
	if dom.OS == nil || dom.OS.NVRam == nil {
		return ""
	}
	nvram := dom.OS.NVRam
	if nvram.Source != nil {
		if nvram.Source.File != nil {
			return nvram.Source.File.File
		}
		return ""
	}
	return nvram.NVRam
}

func (n namespace) ownsDomain(name, definition string) (bool, error) {
	var dom libvirtxml.Domain
	if err := dom.Unmarshal(definition); err != nil {
		return false, err
	}
	if !n.ownsDomainName(name) || dom.Name != name {
		return false, nil
	}
	// Undefining a domain also deletes its NVRAM file.
	if domainNVRAMPath(&dom) != fmt.Sprintf("%s/%s_VARS.fd", n.nvramPoolPath(), name) {
		return false, nil
	}
	if dom.Metadata != nil {
		present, owned, err := n.metadataOwnership(definition, "domain")
		if err != nil || present {
			return owned, err
		}
	}
	for _, info := range dom.SysInfo {
		if info.SMBIOS != nil && info.SMBIOS.OEMStrings != nil &&
			slices.Contains(info.SMBIOS.OEMStrings.Entry, "virtdeploy:1") {
			return true, nil
		}
	}
	return false, nil
}

func (n namespace) ownsNetwork(name, definition string) (bool, error) {
	var network libvirtxml.Network
	if err := network.Unmarshal(definition); err != nil {
		return false, err
	}
	if name != n.libvirtNetworkName() || network.Name != name {
		return false, nil
	}
	if network.Metadata != nil {
		present, owned, err := n.metadataOwnership(definition, "network")
		if err != nil || present {
			return owned, err
		}
	}
	// Legacy networks have no marker. Require the generated NAT/DNS/DHCP shape.
	if network.Domain == nil || network.Domain.Name != name ||
		network.Forward == nil || network.Forward.Mode != "nat" || len(network.IPs) != 1 {
		return false, nil
	}
	dhcp := network.IPs[0].DHCP
	if dhcp == nil || len(dhcp.Ranges) != 1 {
		return false, nil
	}
	for _, host := range dhcp.Hosts {
		if !n.ownsDomainName(host.Name) {
			return false, nil
		}
	}
	return true, nil
}

func (n namespace) ownsPool(name, definition string) (bool, error) {
	var pool libvirtxml.StoragePool
	if err := pool.Unmarshal(definition); err != nil {
		return false, err
	}
	// Storage pool XML has no metadata. Names alone are ambiguous: foo's NVRAM
	// pool and foo-nvram's disk pool are both named foo-nvram-pool.
	if pool.Type != "dir" || pool.Name != name || pool.Target == nil {
		return false, nil
	}
	return (name == n.storagePoolName() && pool.Target.Path == n.storagePoolPath()) ||
		(name == n.nvramPoolName() && pool.Target.Path == n.nvramPoolPath()), nil
}
