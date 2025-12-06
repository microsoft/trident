package netfinder

import (
	"bytes"
	"errors"
	"fmt"
	"net"

	log "github.com/sirupsen/logrus"
	"github.com/vishvananda/netlink"
)

// Finds the IP of the local interface that can reach the target IP.
func FindLocalIpForTargetIp(target string) (string, error) {
	ip := net.ParseIP(target)
	if ip == nil {
		return "", fmt.Errorf("failed to parse target IP: %s", target)
	}

	routes, err := netlink.RouteGet(ip)
	if err != nil {
		return "", fmt.Errorf("failed to get routes: %v", err)
	}

	if len(routes) == 0 {
		return "", errors.New("failed to find route to target")
	}

	link, err := netlink.LinkByIndex(routes[0].LinkIndex)
	if err != nil {
		return "", fmt.Errorf("failed to get link by index: %v", err)
	}

	addrs, err := netlink.AddrList(link, netlink.FAMILY_V4)
	if err != nil {
		return "", fmt.Errorf("failed to get addresses: %v", err)
	}

	if len(addrs) == 0 {
		return "", fmt.Errorf("no addresses found")
	}

	return addrs[0].IPNet.IP.String(), nil
}

// FindDefaultOutboundInterface attempts to find the default interface on the host.
func FindDefaultOutboundInterface() (netlink.Link, error) {
	// Strategy:
	// 1. Enumerate IPv4 routes and look for the default route (Dst == nil).
	// 2. Prefer a default route that has a gateway (Gw != nil).
	// 3. Resolve the link name from the route's LinkIndex.
	// 4. If nothing found for IPv4, attempt IPv6 default as a fallback.

	routes, err := netlink.RouteList(nil, netlink.FAMILY_V4)
	if err != nil {
		return nil, fmt.Errorf("listing routes failed: %w", err)
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
		return nil, errors.New("no default route found")
	}

	link, err := netlink.LinkByIndex(candidate.LinkIndex)
	if err != nil {
		return nil, fmt.Errorf("resolve link by index %d: %w", candidate.LinkIndex, err)
	}

	return link, nil
}

func FindDefaultOutboundIp() (string, error) {
	link, err := FindDefaultOutboundInterface()
	if err != nil {
		return "", fmt.Errorf("failed to find default outbound interface: %w", err)
	}

	addrs, err := netlink.AddrList(link, netlink.FAMILY_V4)
	if err != nil {
		return "", fmt.Errorf("failed to get addresses: %w", err)
	}

	if len(addrs) == 0 {
		return "", fmt.Errorf("no addresses found on interface %s", link.Attrs().Name)
	}

	return addrs[0].IPNet.IP.String(), nil
}
