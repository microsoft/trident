package libvirtutils

import (
	"fmt"
	"net/url"

	"github.com/digitalocean/go-libvirt"
	"github.com/sirupsen/logrus"
)

// Connect establishes a connection to the local libvirt daemon. It
// returns the libvirt connection instance or an error if the connection could
// not be established.
func Connect() (*libvirt.Libvirt, error) {
	parsedURL, err := url.Parse("qemu:///system")
	if err != nil {
		return nil, fmt.Errorf("failed to parse libvirt URI: %w", err)
	}

	logrus.Debugf("Connecting to libvirt at '%s'", parsedURL.String())
	lvConn, err := libvirt.ConnectToURI(parsedURL)
	if err != nil {
		return nil, fmt.Errorf("failed to connect to libvirt hypervisor '%s', Is your user in the libvirt group?: %w", parsedURL.String(), err)
	}

	return lvConn, nil
}
