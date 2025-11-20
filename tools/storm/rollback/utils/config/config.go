package config

type TestConfig struct {
	ArtifactsDir   string `help:"Directory containing artifacts for the VM" default:"/tmp"`
	OutputPath     string `help:"Path to the output directory for logs and artifacts" default:"./output"`
	Verbose        bool   `help:"Enable verbose logging" default:"false"`
	HostConfig     string `help:"Host Configuration to use for updates" default:"./input/trident.yaml"`
	ExtensionName  string `help:"Extension Name to test" default:"testextension"`
	FileServerPort int    `help:"Port for the cosi and extension file server" default:"8000"`
	BuildId        string `help:"Build ID for the VM" default:""`
	ExpectedVolume string `help:"Expected active volume after update" default:"volume-a"`
	ForceCleanup   bool   `help:"Force cleanup of VM when test finishes" default:"false"`
}
