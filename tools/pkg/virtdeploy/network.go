package virtdeploy

import (
	"bytes"
	"errors"
	"fmt"
	"math/big"
	"math/rand"
	"net"

	"github.com/seancfoley/ipaddress-go/ipaddr"
	log "github.com/sirupsen/logrus"
	"github.com/vishvananda/netlink"
	"libvirt.org/go/libvirtxml"
)

const (
	AutoDetectNatInterface = "auto"
)

type ipv4Iterator = ipaddr.Iterator[*ipaddr.IPv4Address]

type virtDeployNetwork struct {
	// Namespace to create resources in
	name          string
	ipNet         *ipaddr.IPv4Address
	leaseIterator ipv4Iterator
	gatewayIP     net.IP
	natInterface  string
	hostAddresses uint64
	leases        []lease
}

type lease struct {
	name string
	ip   net.IP
	mac  macAddress
}

func newVirtDeployNetwork(name string, stdIpNet net.IPNet, natInterface string) (*virtDeployNetwork, error) {
	ipNet, err := ipaddr.NewIPAddressFromNetIPNet(&stdIpNet)
	if err != nil {
		return nil, fmt.Errorf("invalid network: %w", err)
	}

	// Only IPv4 is supported
	if !ipNet.IsIPv4() {
		return nil, fmt.Errorf("only IPv4 networks are supported")
	}

	ipNetV4 := ipNet.ToIPv4()
	if ipNetV4 == nil {
		return nil, fmt.Errorf("failed to convert network to IPv4")
	}

	// Ensure the network has at least 4 addresses
	if ipNetV4.GetCount().Cmp(big.NewInt(4)) < 0 {
		return nil, fmt.Errorf("network is too small, must have at least 4 addresses")
	}

	// Auto-detect the NAT interface if requested
	if natInterface == AutoDetectNatInterface {
		log.Debug("Auto-detecting NAT interface")
		var err error
		natInterface, err = autoDetectNatInterface()
		if err != nil {
			return nil, fmt.Errorf("auto-detect NAT interface: %w", err)
		}

		log.Infof("Auto-detected NAT interface: %s", natInterface)
	}

	// Compute usable host addresses from the total count (subtract network and broadcast)
	total := ipNet.GetCount().Uint64()
	usable := uint64(0)
	if total >= 2 {
		usable = total - 2
	}

	iterator := ipNetV4.Iterator()
	// Skip the network address
	_ = iterator.Next()

	network := &virtDeployNetwork{
		name:          name,
		ipNet:         ipNetV4,
		leaseIterator: iterator,
		natInterface:  natInterface,
		hostAddresses: usable,
		leases:        make([]lease, 0),
	}

	network.gatewayIP = iterator.Next().GetNetIP()
	if net.IPv4zero.Equal(network.gatewayIP) {
		return nil, fmt.Errorf("failed to allocate gateway IP")
	}

	return network, nil
}

func (n *virtDeployNetwork) CIDR() string {
	return n.ipNet.String()
}

// lease returns the next available IP address in the network.
// It returns an error if there are no more available addresses.
func (n *virtDeployNetwork) lease(name string, mac macAddress) (net.IP, error) {
	next := n.leaseIterator.Next()
	if next == nil {
		return nil, fmt.Errorf("no more available IP addresses in the network")
	}

	ip := next.GetNetIP()

	n.leases = append(n.leases, lease{
		name: name,
		ip:   ip,
		mac:  mac,
	})

	log.Tracef("Leased IP %s to %s (MAC %s)", ip, name, mac.String())

	return ip, nil
}

func (n *virtDeployNetwork) asXml() (string, error) {
	hosts := make([]libvirtxml.NetworkDHCPHost, len(n.leases))
	for i, lease := range n.leases {
		hosts[i] = libvirtxml.NetworkDHCPHost{
			IP:   lease.ip.String(),
			MAC:  lease.mac.String(),
			Name: lease.name,
		}
	}

	network := libvirtxml.Network{
		Name: n.name,
		Forward: &libvirtxml.NetworkForward{
			Mode: "nat",
			Dev:  n.natInterface,
		},
		Domain: &libvirtxml.NetworkDomain{
			Name: n.name,
		},
		IPs: []libvirtxml.NetworkIP{
			{
				Address: n.gatewayIP.String(),
				Netmask: n.ipNet.GetNetworkMask().String(),
				DHCP: &libvirtxml.NetworkDHCP{
					Ranges: []libvirtxml.NetworkDHCPRange{
						{
							// Skip the gateway and allocate from .2 to .(n-1)
							Start: n.ipNet.GetLower().Increment(2).GetNetIP().String(),
							End:   n.ipNet.GetUpper().Increment(-1).GetNetIP().String(),
						},
					},
					Hosts: hosts,
				},
			},
		},
	}

	xmldoc, err := network.Marshal()
	if err != nil {
		return "", fmt.Errorf("marshal network to XML: %w", err)
	}

	return string(xmldoc), nil
}

func networkOffset(base net.IP, offset uint64) net.IP {
	ip := make(net.IP, len(base))
	copy(ip, base)

	for i := len(ip) - 1; i >= 0 && offset > 0; i-- {
		sum := uint64(ip[i]) + (offset & 0xFF)
		ip[i] = byte(sum & 0xFF)
		offset = (offset >> 8) + (sum >> 8)
	}

	return ip
}

func autoDetectNatInterface() (string, error) {
	// Strategy:
	// 1. Enumerate IPv4 routes and look for the default route (Dst == nil).
	// 2. Prefer a default route that has a gateway (Gw != nil).
	// 3. Resolve the link name from the route's LinkIndex.
	// 4. If nothing found for IPv4, attempt IPv6 default as a fallback.

	routes, err := netlink.RouteList(nil, netlink.FAMILY_V4)
	if err != nil {
		return "", fmt.Errorf("listing routes failed: %w", err)
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
		return "", errors.New("no default route found")
	}

	link, err := netlink.LinkByIndex(candidate.LinkIndex)
	if err != nil {
		return "", fmt.Errorf("resolve link by index %d: %w", candidate.LinkIndex, err)
	}

	return link.Attrs().Name, nil
}

type macAddress [6]byte

func NewRandomMacAddress(roota, rootb, rootc byte) macAddress {
	var mac macAddress
	mac[0] = roota
	mac[1] = rootb
	mac[2] = rootc
	mac[3] = byte(rand.Intn(256))
	mac[4] = byte(rand.Intn(256))
	mac[5] = byte(rand.Intn(256))

	return mac
}

func (m macAddress) String() string {
	return fmt.Sprintf("%02x:%02x:%02x:%02x:%02x:%02x",
		m[0], m[1], m[2], m[3], m[4], m[5])
}
