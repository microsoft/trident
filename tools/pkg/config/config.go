package config

import "tridenttools/pkg/bmc"

type NetLaunchConfig struct {
	Netlaunch NetlaunchConfigInner `yaml:"netlaunch"`
	Iso       IsoConfig            `yaml:"iso,omitempty"`
}

type NetlaunchConfigInner struct {
	AnnounceIp   *string  `yaml:"announceIp,omitempty"`
	AnnouncePort *uint16  `yaml:"announcePort,omitempty"`
	Bmc          *bmc.Bmc `yaml:"bmc,omitempty"`
	LocalVmUuid  *string  `yaml:"localVmUuid,omitempty"`
	LocalVmNvRam *string  `yaml:"localVmNvRam,omitempty"`
}

type IsoConfig struct {
	PreTridentScript *string `yaml:"preTridentScript,omitempty"`
	ServiceOverride  *string `yaml:"serviceOverride,omitempty"`
}

type NetListenConfig struct {
	Netlisten struct {
		Bmc *bmc.Bmc `yaml:"bmc,omitempty"`
	}
}
