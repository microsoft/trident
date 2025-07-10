package config

import "tridenttools/pkg/bmc"

type NetLaunchConfig struct {
	Netlaunch struct {
		AnnounceIp   *string
		AnnouncePort *uint16
		Bmc          *bmc.Bmc
		LocalVmUuid  *string
	}
	Iso struct {
		PreTridentScript *string
	}
}
