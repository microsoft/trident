package config

type TestConfig struct {
	ArtifactsDir         string `help:"Directory containing artifacts for the VM" default:"/tmp"`
	OutputPath           string `help:"Path to the output directory for logs and artifacts" default:"./output"`
	Verbose              bool   `help:"Enable verbose logging" default:"false"`
	HostConfig           string `help:"Host Configuration to use for updates" default:"./input/trident.yaml"`
	ExtensionName        string `help:"Extension Name to test" default:"test-sysext"`
	FileServerPort       int    `help:"Port for the cosi and extension file server" default:"8000"`
	ExpectedVolume       string `help:"Expected active volume after update" default:"volume-a"`
	ForceCleanup         bool   `help:"Force cleanup of VM when test finishes" default:"false"`
	ImageCustomizerImage string `help:"Image Customizer version to use" default:"mcr.microsoft.com/azurelinux/imagecustomizer:latest"`
	DebugPassword        string `help:"Debug password for the VM" default:""`
	SkipRuntimeUpdates   bool   `help:"Skip runtime updates during the test" default:"false"`
	SkipManualRollbacks  bool   `help:"Skip manual rollbacks during the test" default:"false"`
	SkipExtensionTesting bool   `help:"Skip extension testing during the test" default:"false"`
}
