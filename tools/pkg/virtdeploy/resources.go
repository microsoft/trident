package virtdeploy

import (
	"fmt"
	"net/url"
	"os"
	"path"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	log "github.com/sirupsen/logrus"
)

const (
	FIRMWARE_LOADER_PATH            = "/usr/share/OVMF/OVMF_CODE_4M.fd"
	FIRMWARE_LOADER_SECUREBOOT_PATH = "/usr/share/OVMF/OVMF_CODE_4M.ms.fd"
	FIRMWARE_VARS_PATH              = "/usr/share/OVMF/OVMF_VARS_4M.fd"
	FIRMWARE_VARS_SECUREBOOT_PATH   = "/usr/share/OVMF/OVMF_VARS_4M.ms.fd"
)

type virtDeployResourceConfig struct {
	namespace namespace
	network   *virtDeployNetwork
	pool      storagePool
	nvramPool storagePool
	vms       []VirtDeployVM
	lv        *libvirt.Libvirt
}

func newVirtDeployResourceConfig(config VirtDeployConfig) (*virtDeployResourceConfig, error) {
	// Instantiate the namespace object, which will help with naming resources
	ns := namespace(config.Namespace)

	// Create the network configuration
	network, err := newVirtDeployNetwork(
		ns.libvirtNetworkName(),
		config.IPNet,
		config.NatInterface,
	)
	if err != nil {
		return nil, fmt.Errorf("failed to create network config: %w", err)
	}

	// Connect to libvirt
	lvConn, err := connect()
	if err != nil {
		return nil, err
	}

	// Create the storage pool configuration
	storagePool := newPool(ns.storagePoolName(), fmt.Sprintf("/var/lib/libvirt/%s", ns))
	nvramPool := newPool(ns.nvramPoolName(), fmt.Sprintf("/var/lib/%s-nvram", ns))
	nvramPool.mode = "0777"

	// Create the resource config
	r := &virtDeployResourceConfig{
		namespace: ns,
		network:   network,
		pool:      storagePool,
		nvramPool: nvramPool,
		vms:       config.VMs,
		lv:        lvConn,
	}

	// Initialize all VMs
	for i := range r.vms {
		vm := &r.vms[i]
		vm.name = ns.vmName(i)
		vm.mac = NewRandomMacAddress(0x52, 0x54, 0x00)
		lease, err := r.network.lease(vm.name, vm.mac)
		if err != nil {
			return nil, fmt.Errorf("lease IP for VM %s: %w", vm.name, err)
		}
		vm.ipAddr = lease

		vm.nvramFile = fmt.Sprintf("%s_VARS.fd", vm.name)

		if vm.SecureBoot {
			vm.firmwareLoaderPath = FIRMWARE_LOADER_SECUREBOOT_PATH
			vm.firmwareVarsTemplatePath = FIRMWARE_VARS_SECUREBOOT_PATH
		} else {
			vm.firmwareLoaderPath = FIRMWARE_LOADER_PATH
			vm.firmwareVarsTemplatePath = FIRMWARE_VARS_PATH
		}

		// Set up volume configurations for the VM
		vm.volumes = make([]storageVolume, 0, len(vm.Disks))
		for j, diskSize := range vm.Disks {
			// Initialize volume with basic info, path will be filled in once the
			// volume is created in libvirt.
			vol := newSimpleVolume(fmt.Sprintf("%s-volume-%d.qcow2", vm.name, j), diskSize, "qcow2")

			// If this is the first disk and an OS disk path was specified,
			// set it.
			if j == 0 && vm.OsDiskPath != "" {
				vol.osDisk = vm.OsDiskPath
			}

			// Append the volume to the VM's list of volumes
			vm.volumes = append(vm.volumes, vol)
		}
	}

	return r, nil
}

func cleanupNamespace(namespaceRaw string) error {
	ns := namespace(namespaceRaw)

	// Connect to libvirt
	lvConn, err := connect()
	if err != nil {
		return err
	}

	rc := &virtDeployResourceConfig{
		namespace: ns,
		lv:        lvConn,
	}

	// Delete all VMs in the namespace
	domains, _, err := rc.lv.ConnectListAllDomains(1, libvirt.ConnectListDomainsActive|libvirt.ConnectListDomainsInactive)
	if err != nil {
		return fmt.Errorf("list domains: %w", err)
	}

	for _, dom := range domains {
		if !ns.isInNamespace(dom.Name) {
			continue
		}

		log.Debugf("Tearing down domain %s", dom.Name)
		err = rc.teardownDomain(dom)
		if err != nil {
			return fmt.Errorf("teardown domain %s: %w", dom.Name, err)
		}
	}

	// Delete all networks in the namespace
	networks, _, err := rc.lv.ConnectListAllNetworks(1, libvirt.ConnectListNetworksActive|libvirt.ConnectListNetworksInactive)
	if err != nil {
		return fmt.Errorf("list networks: %w", err)
	}

	for _, nw := range networks {
		if !ns.isInNamespace(nw.Name) {
			continue
		}

		log.Debugf("Tearing down network %s", nw.Name)
		err = rc.teardownNetwork(nw)
		if err != nil {
			return fmt.Errorf("teardown network %s: %w", nw.Name, err)
		}
	}

	// Delete all storage pools in the namespace
	pools, _, err := rc.lv.ConnectListAllStoragePools(1, libvirt.ConnectListStoragePoolsActive|libvirt.ConnectListStoragePoolsInactive)
	if err != nil {
		return fmt.Errorf("list storage pools: %w", err)
	}

	for _, pool := range pools {
		if !ns.isInNamespace(pool.Name) {
			continue
		}

		log.Debugf("Tearing down storage pool %s", pool.Name)
		err = rc.teardownStoragePool(pool)
		if err != nil {
			return fmt.Errorf("teardown storage pool %s: %w", pool.Name, err)
		}
	}

	return nil
}

func connect() (*libvirt.Libvirt, error) {
	// Connect to libvirt
	parsedURL, err := url.Parse("qemu:///system")
	if err != nil {
		return nil, fmt.Errorf("failed to parse libvirt URI: %w", err)
	}

	log.Debugf("Connecting to libvirt at '%s'", parsedURL.String())
	lvConn, err := libvirt.ConnectToURI(parsedURL)
	if err != nil {
		log.Errorf("Failed to connect to the hypervisor '%s'. Is your user in the libvirt group?", parsedURL.String())
		return nil, fmt.Errorf("failed to connect to libvirt: %w", err)
	}

	return lvConn, nil
}

func (rc *virtDeployResourceConfig) close() {
	if rc.lv != nil && rc.lv.IsConnected() {
		err := rc.lv.Disconnect()
		if err != nil {
			log.Warnf("Failed to disconnect from libvirt: %v", err)
		}
		rc.lv = nil
	}
}

func (rc *virtDeployResourceConfig) construct() (*VirtDeployStatus, error) {
	// Create dirs for the nvram files. Do this ASAP to fail fast if we can't.
	// We need to do this with sudo since /var/lib/libvirt is root-owned.
	// if err := sudoCommand("mkdir", []string{"-p", nvramDir}).Run(); err != nil {
	// 	return fmt.Errorf("failed to create NVRAM directory: %w", err)
	// }
	// if err := sudoCommand("chmod", []string{"o+rx", libvirtDir}).Run(); err != nil {
	// 	return fmt.Errorf("failed to set permissions on %s: %w", libvirtDir, err)
	// }
	// if err := sudoCommand("chmod", []string{"o+rx", qemuDir}).Run(); err != nil {
	// 	return fmt.Errorf("failed to set permissions on %s: %w", qemuDir, err)
	// }
	// if err := sudoCommand("chmod", []string{"o+rx", nvramDir}).Run(); err != nil {
	// 	return fmt.Errorf("failed to set permissions on %s: %w", nvramDir, err)
	// }

	err := rc.setupNetwork()
	if err != nil {
		return nil, fmt.Errorf("failed to set up network: %w", err)
	}

	err = rc.setupStoragePool(&rc.pool)
	if err != nil {
		return nil, fmt.Errorf("failed to set up storage pool: %w", err)
	}

	err = rc.setupStoragePool(&rc.nvramPool)
	if err != nil {
		return nil, fmt.Errorf("failed to set up NVRAM storage pool: %w", err)
	}

	err = rc.setupVms()
	if err != nil {
		return nil, fmt.Errorf("failed to set up VMs: %w", err)
	}

	status := &VirtDeployStatus{
		Namespace:   rc.namespace.String(),
		NetworkCIDR: rc.network.CIDR(),
		VMs:         make([]VirtDeployVMStatus, len(rc.vms)),
	}

	for i, vm := range rc.vms {
		status.VMs[i] = VirtDeployVMStatus{
			Name:       vm.name,
			IPAddress:  vm.ipAddr.String(),
			MACAddress: vm.mac.String(),
			Uuid:       uuid.UUID(vm.domain.UUID),
			NvramPath:  vm.nvramPath,
		}
	}

	return status, nil
}

func (rc *virtDeployResourceConfig) setupNetwork() error {
	// Destroy any existing network with the same name
	err := rc.teardownNetworkByName(rc.network.name)
	if err != nil {
		return fmt.Errorf("teardown existing network: %w", err)
	}

	// Turn the configuration into XML
	networkXML, err := rc.network.asXml()
	if err != nil {
		return fmt.Errorf("generate network XML: %w", err)
	}

	log.Tracef("Defining network with XML:\n%s", networkXML)

	// Define the network in libvirt
	nw, err := rc.lv.NetworkDefineXML(networkXML)
	if err != nil {
		return fmt.Errorf("define network: %w", err)
	}

	// Start the network if it's not already running
	active, err := rc.lv.NetworkIsActive(nw)
	if err != nil {
		return fmt.Errorf("check if network is active: %w", err)
	}

	if active == 0 {
		err = rc.lv.NetworkCreate(nw)
		if err != nil {
			return fmt.Errorf("create network: %w", err)
		}
	}

	// Set the network to autostart
	err = rc.lv.NetworkSetAutostart(nw, 1)
	if err != nil {
		return fmt.Errorf("set network to autostart: %w", err)
	}

	log.Infof("Created and started network '%s'", rc.network.name)

	return nil
}

func (rc *virtDeployResourceConfig) teardownNetworkByName(name string) error {
	network, err := rc.lv.NetworkLookupByName(name)
	if err != nil {
		// Check if the error indicates that the network does not exist
		// If so, we can ignore it.
		lverr, ok := err.(libvirt.Error)
		if ok && lverr.Code == uint32(libvirt.ErrNoNetwork) {
			log.Tracef("Network %s does not exist, skipping deletion", name)
			return nil
		}

		return fmt.Errorf("lookup network %s: %w", name, err)
	}

	log.Debugf("Found existing network '%s', deleting.", network.Name)

	return rc.teardownNetwork(network)
}

func (rc *virtDeployResourceConfig) teardownNetwork(lvNetwork libvirt.Network) error {
	active, err := rc.lv.NetworkIsActive(lvNetwork)
	if err != nil {
		return fmt.Errorf("check if network %s is active: %w", lvNetwork.Name, err)
	}

	if active != 0 {
		log.Tracef("Network %s is active, destroying.", lvNetwork.Name)
		err = rc.lv.NetworkDestroy(lvNetwork)
		if err != nil {
			return fmt.Errorf("destroy network %s: %w", lvNetwork.Name, err)
		}
	}

	err = rc.lv.NetworkUndefine(lvNetwork)
	if err != nil {
		return fmt.Errorf("undefine network %s: %w", lvNetwork.Name, err)
	}

	log.Infof("Deleted existing network '%s'", lvNetwork.Name)

	return nil
}

func (rc *virtDeployResourceConfig) setupStoragePool(pool *storagePool) error {
	// Destroy any existing storage pool with the same name
	err := rc.teardownStoragePoolByName(pool.name)
	if err != nil {
		return fmt.Errorf("teardown existing storage pool: %w", err)
	}

	// Turn the configuration into XML
	poolXML, err := pool.asXml()
	if err != nil {
		return fmt.Errorf("generate storage pool XML: %w", err)
	}

	log.Tracef("Defining storage pool with XML:\n%s", poolXML)

	// Define the storage pool in libvirt
	pool.lvPool, err = rc.lv.StoragePoolDefineXML(poolXML, 0)
	if err != nil {
		return fmt.Errorf("define storage pool: %w", err)
	}

	// Build the storage pool
	err = rc.lv.StoragePoolBuild(pool.lvPool, 0)
	if err != nil {
		return fmt.Errorf("build storage pool: %w", err)
	}

	// Start the storage pool if it's not already running
	active, err := rc.lv.StoragePoolIsActive(pool.lvPool)
	if err != nil {
		return fmt.Errorf("check if storage pool is active: %w", err)
	}

	if active == 0 {
		err = rc.lv.StoragePoolCreate(pool.lvPool, 0)
		if err != nil {
			return fmt.Errorf("create storage pool: %w", err)
		}
	}

	// Set the storage pool to autostart
	err = rc.lv.StoragePoolSetAutostart(pool.lvPool, 1)
	if err != nil {
		return fmt.Errorf("set storage pool to autostart: %w", err)
	}

	log.Infof("Created and started storage pool '%s'", pool.lvPool.Name)

	return nil
}

func (rc *virtDeployResourceConfig) teardownStoragePoolByName(name string) error {
	pool, err := rc.lv.StoragePoolLookupByName(name)
	if err != nil {
		// Check if the error indicates that the pool does not exist
		// If so, we can ignore it.
		lverr, ok := err.(libvirt.Error)
		if ok && lverr.Code == uint32(libvirt.ErrNoStoragePool) {
			log.Tracef("Storage pool %s does not exist, skipping deletion", name)
			return nil
		}

		return fmt.Errorf("lookup storage pool %s: %w", name, err)
	}

	log.Debugf("Found existing storage pool '%s', deleting.", pool.Name)

	return rc.teardownStoragePool(pool)
}

func (rc *virtDeployResourceConfig) teardownStoragePool(lvPool libvirt.StoragePool) error {

	vols, _, err := rc.lv.StoragePoolListAllVolumes(lvPool, 1, 0)
	if err != nil {
		return fmt.Errorf("list volumes in storage pool %s: %w", lvPool.Name, err)
	}

	for _, vol := range vols {
		log.Debugf("Tearing down volume %s in storage pool %s", vol.Name, lvPool.Name)
		err = rc.teardownVolume(lvPool, vol.Name)
		if err != nil {
			return fmt.Errorf("teardown volume %s in pool %s: %w", vol.Name, lvPool.Name, err)
		}
	}

	active, err := rc.lv.StoragePoolIsActive(lvPool)
	if err != nil {
		return fmt.Errorf("check if storage pool %s is active: %w", lvPool.Name, err)
	}

	if active != 0 {
		log.Tracef("Storage pool %s is active, destroying.", lvPool.Name)
		err = rc.lv.StoragePoolDestroy(lvPool)
		if err != nil {
			return fmt.Errorf("destroy storage pool %s: %w", lvPool.Name, err)
		}
	}

	err = rc.lv.StoragePoolUndefine(lvPool)
	if err != nil {
		return fmt.Errorf("undefine storage pool %s: %w", lvPool.Name, err)
	}

	log.Infof("Deleted existing storage pool '%s'", lvPool.Name)

	return nil
}

func (rc *virtDeployResourceConfig) setupVms() error {
	for i := range rc.vms {
		err := rc.setupVm(&rc.vms[i])
		if err != nil {
			return fmt.Errorf("setup VM %d: %w", i, err)
		}
	}

	return nil
}

func (rc *virtDeployResourceConfig) setupVm(vm *VirtDeployVM) error {
	// Ensure the storage pools are set up
	if rc.pool.lvPool == (libvirt.StoragePool{}) {
		return fmt.Errorf("storage pool is not set up")
	}

	if rc.nvramPool.lvPool == (libvirt.StoragePool{}) {
		return fmt.Errorf("NVRAM storage pool is not set up")
	}

	// Destroy any existing domain with the same name
	err := rc.teardownDomainByName(vm.name)
	if err != nil {
		return fmt.Errorf("teardown existing domain: %w", err)
	}

	// nvram volume
	vol := newSimpleVolumeMode(vm.nvramFile, 0, "raw", "0666")
	err = rc.setupVolume(&vol, rc.nvramPool)
	if err != nil {
		return fmt.Errorf("setup NVRAM volume: %w", err)
	}

	// Update the VM's NVRAM path to the created volume's path
	vm.nvramPath = vol.path

	// Upload NVRAM template
	log.Debugf("Uploading NVRAM template from %s to %s", vm.firmwareVarsTemplatePath, vol.path)
	err = rc.uploadFileToVolume(vol.lvVol, vm.firmwareVarsTemplatePath)
	if err != nil {
		return fmt.Errorf("upload NVRAM template to volume: %w", err)
	}

	for i := range vm.volumes {
		vol := &vm.volumes[i]
		vol.device = fmt.Sprintf("sd%c", 'a'+i) // /dev/sda, /dev/sdb, etc.
		err := rc.setupVolume(vol, rc.pool)
		if err != nil {
			return fmt.Errorf("setup volume for disk #%d: %w", i+1, err)
		}
	}

	// Initialize cdroms, first add an empty bay.
	// Device name will get filled in later.
	vm.cdroms = []cdrom{
		{},
	}
	if vm.CloudInit != nil {
		// If cloud-init config is provided, create a cloud-init ISO and add it
		// as a CDROM
		tmpDir, err := os.MkdirTemp("", "virtdeploy-cloudinit-")
		if err != nil {
			return fmt.Errorf("create temp dir for cloud init ISO: %w", err)
		}
		defer os.RemoveAll(tmpDir)

		isoPath := path.Join(tmpDir, "cloud-init.iso")
		err = buildCloudInitIso(vm.CloudInit, isoPath)
		if err != nil {
			return fmt.Errorf("build cloud init ISO: %w", err)
		}

		vol := newSimpleVolume(fmt.Sprintf("%s-cloudinit.iso", vm.name), 1, "iso")

		err = rc.setupVolume(&vol, rc.pool)
		if err != nil {
			return fmt.Errorf("setup cloud init volume: %w", err)
		}

		err = rc.uploadFileToVolume(vol.lvVol, isoPath)
		if err != nil {
			return fmt.Errorf("upload cloud init ISO to volume: %w", err)
		}

		vm.cdroms = append(vm.cdroms, cdrom{
			path: vol.path,
		})
	}

	if 'z'-len(vm.cdroms) < 'a'+len(vm.volumes) {
		return fmt.Errorf("too many disks and CDROMs, cannot assign device names")
	}

	// Assign device names to the CDROMs, starting from the end of the alphabet
	// to avoid conflicts with disk device names.
	// e.g. if there are 3 disks (sda, sdb, sdc), CDROMs will be sdz, sdy, sdx, etc.
	for i := range vm.cdroms {
		vm.cdroms[i].device = fmt.Sprintf("sd%c", 'z'-i) // /dev/sdz, /dev/sdy, etc.
	}

	// Turn the configuration into XML
	domainXML, err := vm.asXml(rc.network, rc.nvramPool)
	if err != nil {
		return fmt.Errorf("generate domain XML: %w", err)
	}

	log.Tracef("Defining domain with XML:\n%s", domainXML)

	// Define the domain in libvirt
	vm.domain, err = rc.lv.DomainDefineXMLFlags(domainXML, libvirt.DomainDefineValidate)
	if err != nil {
		return fmt.Errorf("define domain: %w", err)
	}

	log.Infof("Created domain '%s'", vm.name)

	return nil
}

func (rc *virtDeployResourceConfig) teardownDomainByName(name string) error {
	dom, err := rc.lv.DomainLookupByName(name)
	if err != nil {
		// Check if the error indicates that the domain does not exist
		// If so, we can ignore it.
		lverr, ok := err.(libvirt.Error)
		if ok && lverr.Code == uint32(libvirt.ErrNoDomain) {
			log.Tracef("Domain %s does not exist, skipping deletion", name)
			return nil
		}

		return fmt.Errorf("lookup domain %s: %w", name, err)
	}

	log.Debugf("Found existing domain '%s', deleting.", name)
	return rc.teardownDomain(dom)
}

func (rc *virtDeployResourceConfig) teardownDomain(dom libvirt.Domain) error {
	active, err := rc.lv.DomainIsActive(dom)
	if err != nil {
		return fmt.Errorf("check if domain %s is active: %w", dom.Name, err)
	}

	if active != 0 {
		log.Tracef("Domain %s is active, destroying.", dom.Name)
		err = rc.lv.DomainDestroy(dom)
		if err != nil {
			return fmt.Errorf("destroy domain %s: %w", dom.Name, err)
		}
	}

	err = rc.lv.DomainUndefineFlags(dom, libvirt.DomainUndefineNvram)
	if err != nil {
		return fmt.Errorf("undefine domain %s: %w", dom.Name, err)
	}

	log.Infof("Deleted existing domain '%s'", dom.Name)

	return nil
}

func (rc *virtDeployResourceConfig) setupVolume(vol *storageVolume, pool storagePool) error {
	vol.path = fmt.Sprintf("%s/%s", pool.path, vol.name)
	log.Debugf("Setting up volume '%s' at path '%s'", vol.name, vol.path)

	// First, delete any existing volume with the same name
	err := rc.teardownVolume(pool.lvPool, vol.name)
	if err != nil {
		return fmt.Errorf("teardown existing volume: %w", err)
	}

	xml, err := vol.asXml()
	if err != nil {
		return fmt.Errorf("generate volume XML: %w", err)
	}

	log.Tracef("Defining volume with XML:\n%s", xml)

	// Define the volume in libvirt
	vol.lvVol, err = rc.lv.StorageVolCreateXML(pool.lvPool, xml, 0)
	if err != nil {
		return fmt.Errorf("create volume %s: %w", vol.name, err)
	}

	if vol.osDisk != "" {
		log.Infof("Uploading OS disk image '%s' to volume '%s'", vol.osDisk, vol.name)
		err = rc.uploadFileToVolume(vol.lvVol, vol.osDisk)
		if err != nil {
			return fmt.Errorf("upload OS disk to volume %s: %w", vol.name, err)
		}
	} else {
		log.Debugf("No OS disk specified for volume '%s', creating blank disk", vol.name)
	}

	log.Infof("Created volume '%s'", vol.name)

	return nil
}

func (rc *virtDeployResourceConfig) teardownVolume(pool libvirt.StoragePool, name string) error {
	vol, err := rc.lv.StorageVolLookupByName(pool, name)
	if err != nil {
		// Check if the error indicates that the volume does not exist
		// If so, we can ignore it.
		lverr, ok := err.(libvirt.Error)
		if ok && lverr.Code == uint32(libvirt.ErrNoStorageVol) {
			log.Tracef("Volume %s does not exist, skipping deletion", name)
			return nil
		}

		return fmt.Errorf("lookup volume %s: %w", name, err)
	}

	log.Debugf("Found existing volume '%s', deleting.", name)

	err = rc.lv.StorageVolDelete(vol, 0)
	if err != nil {
		return fmt.Errorf("delete volume %s: %w", name, err)
	}

	log.Infof("Deleted existing volume '%s'", name)

	return nil
}

func (rc *virtDeployResourceConfig) uploadFileToVolume(vol libvirt.StorageVol, filePath string) error {
	stat, err := os.Stat(filePath)
	if err != nil {
		return fmt.Errorf("stat file %s: %w", filePath, err)
	}

	fileSize := stat.Size()
	if fileSize == 0 {
		return fmt.Errorf("file %s is empty", filePath)
	}

	log.Debugf("Uploading %d bytes from file '%s' to volume '%s'", fileSize, filePath, vol.Name)

	file, err := os.Open(filePath)
	if err != nil {
		return fmt.Errorf("open file %s: %w", filePath, err)
	}
	defer file.Close()

	err = rc.lv.StorageVolUpload(vol, file, 0, uint64(fileSize), 0)
	if err != nil {
		return fmt.Errorf("upload to volume %s: %w", vol.Name, err)
	}

	log.Infof("Uploaded file '%s' to volume '%s'", filePath, vol.Name)

	return nil
}
