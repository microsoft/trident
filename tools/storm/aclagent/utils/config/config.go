package config

type TestConfig struct {
	ArtifactsDir          string `help:"Directory containing artifacts for the VM" default:"."`
	OutputPath            string `help:"Path to the output directory for logs and artifacts" default:"./output"`
	Verbose               bool   `help:"Enable verbose logging" default:"false"`
	ForceCleanup          bool   `help:"Force cleanup of VM when test finishes" default:"false"`
	APIServerPort         int    `help:"Runner port exposed into VM for fake apiserver" default:"18080"`
	NebraskaPort          int    `help:"Runner port exposed into VM for fake Nebraska endpoint" default:"18081"`
	TargetVersion         string `help:"Target OS image version to request" default:"202507.28.0"`
	NebraskaPackageName   string `help:"Package name returned by the fake Nebraska endpoint (overridden by the image file name when ImagePath is set)" default:"acl.cosi"`
	NebraskaCodebase      string `help:"Base URL returned by the fake Nebraska endpoint (overridden to point at the fake image server when ImagePath is set)" default:"https://example.invalid/images/"`
	NebraskaSHA384        string `help:"SHA384 returned by the fake Nebraska endpoint (overridden by the real hash of ImagePath when set)" default:"111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111"`
	ImagePath             string `help:"Path to a real OS update image (e.g. a .cosi file) to serve to tridentd during staging; when set, this takes precedence over NebraskaCodebase/NebraskaPackageName/NebraskaSHA384" default:"artifacts/trident-vm-acl-agent-update-testimage.cosi"`
	ImageServerPort       int    `help:"Runner port exposed into VM for the fake image server" default:"18082"`
	NodeName              string `help:"Node name served by the fake apiserver" default:"trident-node"`
	HostEndpointIP        string `help:"Host IP the VM can reach the fake apiserver/Nebraska endpoints at" default:"192.168.122.1"`
	ExpectedInitialVolume string `help:"Expected active volume immediately after deployment" default:"volume-a"`
}
