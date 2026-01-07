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
		return fmt.Errorf("failed to create VM resources: %w", err)
	}

	log.Debugf("Connecting to libvirt at '%s'", parsedURL.String())
	lvConn, err := libvirt.ConnectToURI(parsedURL)
	if err != nil {
		log.Errorf("Failed to connect to the hypervisor '%s'. Is your user in the libvirt group?", parsedURL.String())
		return fmt.Errorf("failed to connect to libvirt: %w", err)
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
