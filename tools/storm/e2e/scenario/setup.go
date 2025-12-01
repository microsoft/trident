package scenario

import (
	"fmt"
	"net"
	"tridenttools/pkg/config"
	"tridenttools/pkg/ref"
	"tridenttools/pkg/virtdeploy"

	"github.com/microsoft/storm"
	log "github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type testHostInfo interface {
	IPAddress() net.IP
	NetlaunchConfig() []byte
	Cleanup() error
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

	// Netlaunch can figure out everything but the VM UUID itself.
	config := config.NetLaunchConfig{
		Netlaunch: config.NetlaunchConfigInner{
			LocalVmUuid:  ref.Of(status.VMs[0].Uuid.String()),
			LocalVmNvRam: &status.VMs[0].NvramPath,
		},
	}

	yamlData, err := yaml.Marshal(config)
	if err != nil {
		return fmt.Errorf("failed to marshal YAML: %w", err)
	}

	s.testHost = &testHostVirtDeploy{
		vm:        status.VMs[0],
		netlaunch: yamlData,
		namespace: status.Namespace,
	}

	return nil
}

type testHostVirtDeploy struct {
	namespace string
	vm        virtdeploy.VirtDeployVMStatus
	netlaunch []byte
}

func (t *testHostVirtDeploy) IPAddress() net.IP {
	return net.ParseIP(t.vm.IPAddress)
}

func (t *testHostVirtDeploy) NetlaunchConfig() []byte {
	return t.netlaunch
}

func (t *testHostVirtDeploy) Cleanup() error {
	log.Infof("Cleaning virtdeploy resources in namespace %s", t.namespace)
	return virtdeploy.DeleteResources(t.namespace)
}
