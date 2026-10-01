package scenario

import (
	"fmt"
	"net"
	"net/url"

	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/ref"
	"tridenttools/pkg/virtdeploy"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	log "github.com/sirupsen/logrus"
	"libvirt.org/go/libvirtxml"
)

// defaultDataDiskSizeGB is the size (GB) virtdeploy creates the VM data disks
// at (see setupTestHostVm Disks); used when replacing a failed RAID member disk.
const defaultDataDiskSizeGB uint = 32

type testHostInfo interface {
	// Retrieve the IP address of the test host.
	IPAddress() net.IP

	// Retrieve the netlaunch connection configuration for the test host.
	NetlaunchConnectionConfig() netlaunch.HostConnectionConfiguration

	// Cleans up the test host resources.
	Cleanup() error

	// When the test host is a VM, retrieve additional VM info. Returns nil
	// otherwise.
	VmInfo() testVmHostInfo
}

type testVmHostInfo interface {
	// Returns the libvirt connection instance.
	Lv() *libvirt.Libvirt

	// Returns the VM UUID.
	VmUuid() uuid.UUID

	// Returns the name of the VM.
	VmName() string

	// Returns the XML definition of the VM.
	VmXml() *libvirtxml.Domain

	// Returns the serial log file path of the VM.
	SerialLogPath() (string, error)

	// Returns the libvirt DOMAIN object for the VM.
	LvDomain() libvirt.Domain

	// FailAndReplaceDataDisk simulates a failed RAID member disk: it forcibly
	// powers off the VM, deletes the given data disk volume, recreates it blank
	// at the same path/size, and powers the VM back on. diskIndex is the 0-based
	// disk index (disk 0 is the OS disk; RAID member disks start at 1).
	FailAndReplaceDataDisk(diskIndex uint) error
}

func (s *TridentE2EScenario) setupTestHost(tc storm.TestCase) error {
	var err error
	switch s.hardware {
	case HardwareTypeVM:
		err = s.setupTestHostVm(tc)
	default:
		err = fmt.Errorf("hardware type not implemented: %s", s.hardware.ToString())
	}

	return err
}

func (s *TridentE2EScenario) setupTestHostVm(tc storm.TestCase) error {
	parsedURL, err := url.Parse("qemu:///system")
	if err != nil {
		return fmt.Errorf("failed to parse libvirt URI: %w", err)
	}

	log.Debugf("Connecting to libvirt at '%s'", parsedURL.String())
	lvConn, err := libvirt.ConnectToURI(parsedURL)
	if err != nil {
		log.Errorf("Failed to connect to the hypervisor '%s'. Is your user in the libvirt group?", parsedURL.String())
		return fmt.Errorf("failed to connect to libvirt: %w", err)
	}

	_, ipNet, err := net.ParseCIDR("192.168.242.0/24")
	if err != nil {
		return fmt.Errorf("failed to parse CIDR: %w", err)
	}

	status, err := virtdeploy.CreateResources(virtdeploy.VirtDeployConfig{
		Namespace: "trident-e2e-" + s.name,
		IPNet:     *ipNet,
		VMs: []virtdeploy.VirtDeployVM{
			{
				Cpus:        4,
				Mem:         12,
				Disks:       []uint{32, 32},
				EmulatedTPM: true,
				SecureBoot:  true,
			},
		},
	})
	if err != nil {
		lvConn.Disconnect()
		return fmt.Errorf("failed to create VM resources: %w", err)
	}

	s.testHost = &testHostVirtDeploy{
		vm:        status.VMs[0],
		namespace: status.Namespace,
		connectionConfig: netlaunch.HostConnectionConfiguration{
			LocalVmUuid:  ref.Of(status.VMs[0].Uuid.String()),
			LocalVmNvRam: &status.VMs[0].NvramPath,
		},
		lv: lvConn,
	}

	return nil
}

type testHostVirtDeploy struct {
	namespace        string
	vm               virtdeploy.VirtDeployVMStatus
	connectionConfig netlaunch.HostConnectionConfiguration

	// Libvirt connection instance.
	lv *libvirt.Libvirt
}

func (t *testHostVirtDeploy) IPAddress() net.IP {
	return net.ParseIP(t.vm.IPAddress)
}

func (t *testHostVirtDeploy) NetlaunchConnectionConfig() netlaunch.HostConnectionConfiguration {
	return t.connectionConfig
}

func (t *testHostVirtDeploy) Cleanup() error {
	log.Infof("Cleaning virtdeploy resources in namespace %s", t.namespace)

	if t.lv != nil && t.lv.IsConnected() {
		err := t.lv.Disconnect()
		if err != nil {
			log.WithError(err).Warn("failed to close libvirt connection")
		}
	}

	return virtdeploy.DeleteResources(t.namespace)
}

func (t *testHostVirtDeploy) VmInfo() testVmHostInfo {
	return t
}

func (t *testHostVirtDeploy) Lv() *libvirt.Libvirt {
	return t.lv
}

func (t *testHostVirtDeploy) LvDomain() libvirt.Domain {
	return t.vm.Domain
}

func (t *testHostVirtDeploy) VmUuid() uuid.UUID {
	return t.vm.Uuid
}

func (t *testHostVirtDeploy) VmName() string {
	return t.vm.Name
}

func (t *testHostVirtDeploy) VmXml() *libvirtxml.Domain {
	return t.vm.Definition
}

func (t *testHostVirtDeploy) SerialLogPath() (string, error) {
	if t.vm.Definition == nil {
		// Should never happen, but guard just in case
		return "", fmt.Errorf("VM definition is nil")
	}

	for _, console := range t.vm.Definition.Devices.Consoles {
		if console.Log != nil {
			logrus.Debugf("VM serial log file path: %s", console.Log.File)
			return console.Log.File, nil
		}
	}

	return "", fmt.Errorf("failed to find a serial device with a log backend in VM definition %s", t.vm.Name)
}

// storagePoolName returns the libvirt storage pool name for this VM's
// namespace, matching virtdeploy's naming convention (<namespace>-pool).
func (t *testHostVirtDeploy) storagePoolName() string {
	return t.namespace + "-pool"
}

// FailAndReplaceDataDisk simulates a failed RAID member disk: it forcibly
// powers off the VM, deletes the data disk volume at diskIndex, recreates it
// blank at the same path and size, and powers the VM back on. This drives the
// rebuild-raid test the same way the legacy helper did (via virsh + qemu-img),
// but through the libvirt API.
func (t *testHostVirtDeploy) FailAndReplaceDataDisk(diskIndex uint) error {
	// virtdeploy names disk volumes "<vmName>-volume-<index>.qcow2".
	volName := fmt.Sprintf("%s-volume-%d.qcow2", t.vm.Name, diskIndex)

	pool, err := t.lv.StoragePoolLookupByName(t.storagePoolName())
	if err != nil {
		return fmt.Errorf("failed to look up storage pool %q: %w", t.storagePoolName(), err)
	}

	vol, err := t.lv.StorageVolLookupByName(pool, volName)
	if err != nil {
		return fmt.Errorf("failed to look up disk volume %q: %w", volName, err)
	}
	volPath, err := t.lv.StorageVolGetPath(vol)
	if err != nil {
		return fmt.Errorf("failed to get path of disk volume %q: %w", volName, err)
	}

	// Determine the disk's declared size (GB). virtdeploy creates the VM's data
	// disks at a fixed size, so use that constant for the replacement.
	diskSizeGB := uint(defaultDataDiskSizeGB)

	// Force the VM off (a failed disk is an abrupt event, not a clean shutdown).
	active, err := t.lv.DomainIsActive(t.vm.Domain)
	if err != nil {
		return fmt.Errorf("failed to check whether VM %q is active: %w", t.vm.Name, err)
	}
	if active != 0 {
		logrus.Infof("Powering off VM %q to replace data disk %q", t.vm.Name, volName)
		if err := t.lv.DomainDestroy(t.vm.Domain); err != nil {
			return fmt.Errorf("failed to power off VM %q: %w", t.vm.Name, err)
		}
	}

	logrus.Infof("Deleting data disk volume %q", volName)
	if err := t.lv.StorageVolDelete(vol, 0); err != nil {
		return fmt.Errorf("failed to delete disk volume %q: %w", volName, err)
	}

	newVolXml := libvirtxml.StorageVolume{
		Name:     volName,
		Capacity: &libvirtxml.StorageVolumeSize{Unit: "G", Value: uint64(diskSizeGB)},
		Target: &libvirtxml.StorageVolumeTarget{
			Path:        volPath,
			Format:      &libvirtxml.StorageVolumeTargetFormat{Type: "qcow2"},
			Permissions: &libvirtxml.StorageVolumeTargetPermissions{Mode: "0644"},
		},
	}
	xml, err := newVolXml.Marshal()
	if err != nil {
		return fmt.Errorf("failed to marshal replacement volume XML: %w", err)
	}
	logrus.Infof("Recreating blank data disk volume %q (%d GB)", volName, diskSizeGB)
	if _, err := t.lv.StorageVolCreateXML(pool, xml, 0); err != nil {
		return fmt.Errorf("failed to recreate disk volume %q: %w", volName, err)
	}

	logrus.Infof("Powering VM %q back on", t.vm.Name)
	if err := t.lv.DomainCreate(t.vm.Domain); err != nil {
		return fmt.Errorf("failed to power on VM %q: %w", t.vm.Name, err)
	}

	return nil
}
