package virtdeploy

import (
	"fmt"
	"net/url"
	"os"

	"github.com/digitalocean/go-libvirt"
	log "github.com/sirupsen/logrus"
)

const (
	libvirtDir = "/var/lib/libvirt"
	qemuDir    = "/var/lib/libvirt/qemu"
	nvramDir   = "/var/lib/libvirt/qemu/nvram"
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
		vm.name = ns.vmName(i + 1)
		vm.mac = NewRandomMacAddress(0x52, 0x54, 0x00)
		lease, err := r.network.lease(vm.name, vm.mac)
		if err != nil {
			return nil, fmt.Errorf("lease IP for VM %s: %w", vm.name, err)
		}
		vm.ipAddr = lease

		// Set up volume configurations for the VM
		vm.volumes = make([]storageVolume, 0, len(vm.Disks))
		for j, diskSize := range vm.Disks {
			// Initialize volume with basic info, path will be filled in once the
			// volume is created in libvirt.
			vol := storageVolume{
				name: fmt.Sprintf("%s-volume-%d.qcow2", vm.name, j+1),
				size: diskSize,
				path: fmt.Sprintf("%s/%s.qcow2", r.pool.path, vm.name),
			}

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

func (rc *virtDeployResourceConfig) close() {
	if rc.lv != nil && rc.lv.IsConnected() {
		err := rc.lv.Disconnect()
		if err != nil {
			log.Warnf("Failed to disconnect from libvirt: %v", err)
		}
		rc.lv = nil
	}
}

func (rc *virtDeployResourceConfig) construct() error {
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
		return fmt.Errorf("failed to set up network: %w", err)
	}

	err = rc.setupStoragePool(&rc.pool)
	if err != nil {
		return fmt.Errorf("failed to set up storage pool: %w", err)
	}

	err = rc.setupStoragePool(&rc.nvramPool)
	if err != nil {
		return fmt.Errorf("failed to set up NVRAM storage pool: %w", err)
	}

	err = rc.setupVms()
	if err != nil {
		return fmt.Errorf("failed to set up VMs: %w", err)
	}

	return nil
}

func (rc *virtDeployResourceConfig) setupNetwork() error {
	// Destroy any existing network with the same name
	err := rc.teardownNetwork(rc.network.name)
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

func (rc *virtDeployResourceConfig) teardownNetwork(name string) error {
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

	active, err := rc.lv.NetworkIsActive(network)
	if err != nil {
		return fmt.Errorf("check if network %s is active: %w", name, err)
	}

	if active != 0 {
		log.Tracef("Network %s is active, destroying.", name)
		err = rc.lv.NetworkDestroy(network)
		if err != nil {
			return fmt.Errorf("destroy network %s: %w", name, err)
		}
	}

	err = rc.lv.NetworkUndefine(network)
	if err != nil {
		return fmt.Errorf("undefine network %s: %w", name, err)
	}

	log.Infof("Deleted existing network '%s'", name)

	return nil
}

func (rc *virtDeployResourceConfig) setupStoragePool(pool *storagePool) error {
	// Destroy any existing storage pool with the same name
	err := rc.teardownStoragePool(pool.name)
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

func (rc *virtDeployResourceConfig) teardownStoragePool(name string) error {
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

	active, err := rc.lv.StoragePoolIsActive(pool)
	if err != nil {
		return fmt.Errorf("check if storage pool %s is active: %w", name, err)
	}

	if active != 0 {
		log.Tracef("Storage pool %s is active, destroying.", name)
		err = rc.lv.StoragePoolDestroy(pool)
		if err != nil {
			return fmt.Errorf("destroy storage pool %s: %w", name, err)
		}
	}

	err = rc.lv.StoragePoolUndefine(pool)
	if err != nil {
		return fmt.Errorf("undefine storage pool %s: %w", name, err)
	}

	log.Infof("Deleted existing storage pool '%s'", name)

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
	err := rc.teardownDomain(vm.name)
	if err != nil {
		return fmt.Errorf("teardown existing domain: %w", err)
	}

	for i := range vm.volumes {
		vol := &vm.volumes[i]
		vol.device = fmt.Sprintf("sd%c", 'a'+i) // /dev/sda, /dev/sdb, etc.
		err := rc.setupVolume(vol, rc.pool)
		if err != nil {
			return fmt.Errorf("setup volume for disk #%d: %w", i+1, err)
		}
	}

	return nil
}

func (rc *virtDeployResourceConfig) teardownDomain(name string) error {
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

	active, err := rc.lv.DomainIsActive(dom)
	if err != nil {
		return fmt.Errorf("check if domain %s is active: %w", name, err)
	}

	if active != 0 {
		log.Tracef("Domain %s is active, destroying.", name)
		err = rc.lv.DomainDestroy(dom)
		if err != nil {
			return fmt.Errorf("destroy domain %s: %w", name, err)
		}
	}

	err = rc.lv.DomainUndefineFlags(dom, libvirt.DomainUndefineNvram)
	if err != nil {
		return fmt.Errorf("undefine domain %s: %w", name, err)
	}

	log.Infof("Deleted existing domain '%s'", name)

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

	return nil
}
