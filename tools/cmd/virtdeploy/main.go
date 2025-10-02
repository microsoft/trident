package main

import (
	"net"
	"tridenttools/pkg/virtdeploy"

	log "github.com/sirupsen/logrus"
)

func main() {
	log.SetLevel(log.TraceLevel)
	err := virtdeploy.CreateResources(virtdeploy.VirtDeployConfig{
		Namespace: "virtdeploy",
		IPNet: net.IPNet{
			IP:   net.IPv4(192, 168, 242, 0),
			Mask: net.CIDRMask(24, 32),
		},
		NatInterface: virtdeploy.AutoDetectNatInterface,
		VMs: []virtdeploy.VirtDeployVM{
			{
				Cpus:        4,
				Mem:         2,
				Disks:       []uint{16},
				SecureBoot:  true,
				EmulatedTPM: true,
				// OsDiskPath:  "go.mod",
			},
		},
	})

	if err != nil {
		log.Fatalf("Failed to create resources: %v", err)
	}
}
