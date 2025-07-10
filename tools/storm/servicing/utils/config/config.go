package config

import (
	"tridenttools/storm/servicing/utils/azure"
	"tridenttools/storm/servicing/utils/qemu"
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

type TestConfig struct {
	ArtifactsDir       string `help:"Directory containing artifacts for the VM" default:"/tmp"`
	OutputPath         string `help:"Path to the output directory for logs and artifacts" default:"./output"`
	Verbose            bool   `help:"Enable verbose logging" default:"false"`
	RetryCount         int    `help:"Number of retry attempts for updates" default:"3"`
	Rollback           bool   `help:"Enable rollback testing" default:"false"`
	RollbackRetryCount int    `help:"Number of retry attempts for updates" default:"3"`
	UpdatePortA        int    `help:"Port for the first update server" default:"8000"`
	UpdatePortB        int    `help:"Port for the second update server" default:"8001"`
	BuildId            string `help:"Build ID for the VM" default:""`
	ExpectedVolume     string `help:"Expected active volume after update" default:"volume-a"`
	ForceCleanup       bool   `help:"Force cleanup of VM when test finishes" default:"false"`
}

type ServicingConfig struct {
	VMConfig    VMConfig
	TestConfig  TestConfig
	QemuConfig  qemu.QemuConfig
	AzureConfig azure.AzureConfig
}
