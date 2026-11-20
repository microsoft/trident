package virtdeploy

import (
	"io"

	"github.com/digitalocean/go-libvirt"
)

type libvirtClient interface {
	IsConnected() bool
	Disconnect() error
	ConnectListAllDomains(int32, libvirt.ConnectListAllDomainsFlags) ([]libvirt.Domain, uint32, error)
	ConnectListAllNetworks(int32, libvirt.ConnectListAllNetworksFlags) ([]libvirt.Network, uint32, error)
	ConnectListAllStoragePools(int32, libvirt.ConnectListAllStoragePoolsFlags) ([]libvirt.StoragePool, uint32, error)
	DomainDefineXMLFlags(string, libvirt.DomainDefineFlags) (libvirt.Domain, error)
	DomainGetXMLDesc(libvirt.Domain, libvirt.DomainXMLFlags) (string, error)
	DomainLookupByName(string) (libvirt.Domain, error)
	DomainIsActive(libvirt.Domain) (int32, error)
	DomainCreate(libvirt.Domain) error
	DomainDestroy(libvirt.Domain) error
	DomainUndefineFlags(libvirt.Domain, libvirt.DomainUndefineFlagsValues) error
	NetworkDefineXML(string) (libvirt.Network, error)
	NetworkGetXMLDesc(libvirt.Network, uint32) (string, error)
	NetworkLookupByName(string) (libvirt.Network, error)
	NetworkIsActive(libvirt.Network) (int32, error)
	NetworkCreate(libvirt.Network) error
	NetworkDestroy(libvirt.Network) error
	NetworkUndefine(libvirt.Network) error
	NetworkSetAutostart(libvirt.Network, int32) error
	StoragePoolDefineXML(string, uint32) (libvirt.StoragePool, error)
	StoragePoolGetXMLDesc(libvirt.StoragePool, libvirt.StorageXMLFlags) (string, error)
	StoragePoolLookupByName(string) (libvirt.StoragePool, error)
	StoragePoolIsActive(libvirt.StoragePool) (int32, error)
	StoragePoolBuild(libvirt.StoragePool, libvirt.StoragePoolBuildFlags) error
	StoragePoolCreate(libvirt.StoragePool, libvirt.StoragePoolCreateFlags) error
	StoragePoolDestroy(libvirt.StoragePool) error
	StoragePoolUndefine(libvirt.StoragePool) error
	StoragePoolSetAutostart(libvirt.StoragePool, int32) error
	StoragePoolListAllVolumes(libvirt.StoragePool, int32, uint32) ([]libvirt.StorageVol, uint32, error)
	StorageVolCreateXML(libvirt.StoragePool, string, libvirt.StorageVolCreateFlags) (libvirt.StorageVol, error)
	StorageVolLookupByName(libvirt.StoragePool, string) (libvirt.StorageVol, error)
	StorageVolDelete(libvirt.StorageVol, libvirt.StorageVolDeleteFlags) error
	StorageVolUpload(libvirt.StorageVol, io.Reader, uint64, uint64, libvirt.StorageVolUploadFlags) error
}
