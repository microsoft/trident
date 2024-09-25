package netfinder

import (
	"errors"
	"fmt"
	"net"

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
