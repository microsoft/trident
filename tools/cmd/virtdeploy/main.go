package main

import (
	"net"
	"tridenttools/pkg/virtdeploy"

	log "github.com/sirupsen/logrus"
)

func main() {
	log.SetLevel(log.DebugLevel)
	err := virtdeploy.CreateResources(virtdeploy.VirtDeployConfig{
		Namespace: "test-namespace",
		IPNet: net.IPNet{
			IP:   net.IPv4(192, 168, 242, 0),
			Mask: net.CIDRMask(24, 32),
		},
		NatInterface: virtdeploy.AutoDetectNatInterface,
	})

	if err != nil {
		log.Fatalf("Failed to create resources: %v", err)
	}
}
