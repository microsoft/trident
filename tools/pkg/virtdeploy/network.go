package virtdeploy

import "net"

type virtDeployNetwork struct {
	// Namespace to create resources in
	name         string
	ipNet        net.IPNet
	natInterface string
}
