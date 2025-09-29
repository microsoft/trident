package virtdeploy

import (
	"bytes"
	"errors"
	"fmt"
	"net"
	"os"

	log "github.com/sirupsen/logrus"
	"github.com/vishvananda/netlink"
)

const (
	AutoDetectNatInterface = "auto"
)

type VirtDeployConfig struct {
	// Namespace to create resources in
	Namespace string

	// Virtual network configuration
	IPNet net.IPNet

	// Interface on the host to give the VM outbound network access.
	// Set to "auto" to automatically detect the default interface.
	// Set to "" to disable outbound network access.
	NatInterface string

	// List of VMs to create
	VMs []VirtDeployVM
}

func (c *VirtDeployConfig) init() error {
	if err := c.autoDetectNatInterface(); err != nil {
		return fmt.Errorf("failed to auto-detect NAT interface: %w", err)
	}

	for i := range c.VMs {
		vm := &c.VMs[i]
		vm.name = fmt.Sprintf("%s-vm-%d", c.Namespace, i)

		if err := vm.validate(); err != nil {
			return fmt.Errorf("VM %d: validation failed: %w", i, err)
		}
	}

	return nil
}

type VirtDeployVM struct {
	name   string
	ipAddr net.IP
	// uuid        uuid.UUID

	// Number of virtual CPUs
	Cpus uint

	// Amount of memory (in GiB)
	Mem uint

	// List of disk sizes (in GiB)
	Disks []uint

	// Optional path to the OS disk to attach to the first disk. If empty, a
	// blank disk will be created.
	OsDiskPath string

	// Optional cloud-init configuration
	CloudInit *CloudInitConfig

	// Configure secure boot firmware
	SecureBoot bool

	// Configure an emulated TPM device
	EmulatedTPM bool
}

func (vm *VirtDeployVM) validate() error {
	if vm.Cpus == 0 {
		return fmt.Errorf("VM %s: CPU count must be > 0", vm.name)
	}
	if vm.Mem == 0 {
		return fmt.Errorf("VM %s: memory must be > 0", vm.name)
	}
	if len(vm.Disks) == 0 {
		return fmt.Errorf("VM %s: at least one disk must be specified", vm.name)
	}
	for j, sz := range vm.Disks {
		if sz == 0 {
			return fmt.Errorf("VM %s: disk %d size must be > 0", vm.name, j)
		}
	}

	if vm.OsDiskPath != "" {
		if _, err := os.Stat(vm.OsDiskPath); err != nil {
			return fmt.Errorf("VM %s: OS disk path is invalid: %w", vm.name, err)
		}
	}

	return nil
}

type CloudInitConfig struct {
	Userdata string
	Metadata string
}

func (c *VirtDeployConfig) autoDetectNatInterface() error {
	if c.NatInterface == "" || c.NatInterface != AutoDetectNatInterface {
		return nil
	}

	log.Debug("Auto-detecting NAT interface")
	// Strategy:
	// 1. Enumerate IPv4 routes and look for the default route (Dst == nil).
	// 2. Prefer a default route that has a gateway (Gw != nil).
	// 3. Resolve the link name from the route's LinkIndex.
	// 4. If nothing found for IPv4, attempt IPv6 default as a fallback.

	routes, err := netlink.RouteList(nil, netlink.FAMILY_V4)
	if err != nil {
		return fmt.Errorf("listing routes failed: %w", err)
	}

	isDefaultNet := func(network *net.IPNet) bool {
		if network == nil {
			return false
		}
		defaultNetwork := net.IPNet{
			IP:   net.IPv4(0, 0, 0, 0),
			Mask: net.CIDRMask(0, 32),
		}
		return network.IP.Equal(defaultNetwork.IP) && bytes.Equal(network.Mask, defaultNetwork.Mask)
	}

	var candidate *netlink.Route
	for i := range routes {
		r := &routes[i]
		log.Tracef("Checking Route: %+v", *r)
		if !isDefaultNet(r.Dst) {
			// Route does not target the default network
			log.Trace("Not a default route, skipping")
			continue
		}

		if r.Gw == nil {
			// Skip routes without a gateway
			log.Trace("No gateway, skipping")
			continue
		}

		candidate = r
		// Good enough
		break
	}

	if candidate == nil {
		return errors.New("no default route found")
	}

	link, err := netlink.LinkByIndex(candidate.LinkIndex)
	if err != nil {
		return fmt.Errorf("resolve link by index %d: %w", candidate.LinkIndex, err)
	}

	log.Infof("Auto-detected NAT interface: %s", link.Attrs().Name)
	c.NatInterface = link.Attrs().Name
	return nil
}
