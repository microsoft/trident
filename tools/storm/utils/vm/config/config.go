package config

import (
	stormvmazure "tridenttools/storm/utils/vm/azure"
	stormvmqemu "tridenttools/storm/utils/vm/qemu"
)

// VMPlatformType represents the test platform (qemu or azure)
type VMPlatformType string

const (
	PlatformQEMU  VMPlatformType = "qemu"
	PlatformAzure VMPlatformType = "azure"
)

type VMConfig struct {
	Name              string         `help:"Name of the VM" default:"trident-vm-verity-test"`
	Platform          VMPlatformType `help:"Platform for the VM (qemu or azure)" default:"qemu"`
	User              string         `help:"User to use for SSH connection" default:"testuser"`
	SshPrivateKeyPath string         `help:"Path to the SSH private key file" default:"~/.ssh/id_rsa"`
}

type AllVMConfig struct {
	VMConfig    VMConfig
	QemuConfig  stormvmqemu.QemuConfig
	AzureConfig stormvmazure.AzureConfig
}
