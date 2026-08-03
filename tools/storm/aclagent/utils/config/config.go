package config

type TestConfig struct {
	ArtifactsDir          string `help:"Directory containing artifacts for the VM" default:"."`
	OutputPath            string `help:"Path to the output directory for logs and artifacts" default:"./output"`
	Verbose               bool   `help:"Enable verbose logging" default:"false"`
	ForceCleanup          bool   `help:"Force cleanup of VM when test finishes" default:"false"`
	APIServerPort         int    `help:"Runner port exposed into VM for fake apiserver" default:"18080"`
	NebraskaPort          int    `help:"Runner port exposed into VM for fake Nebraska endpoint" default:"18081"`
	RebootMarkerFile      string `help:"Runner-side marker file path for intercepted reboots" default:"./output/trident-acl-agent-reboot-marker"`
	RebootDurationSeconds int    `help:"Simulated reboot not-ready duration in seconds" default:"30"`
	TargetVersion         string `help:"Target OS image version to request" default:"202507.28.0"`
	NebraskaPackageName   string `help:"Package name returned by the fake Nebraska endpoint" default:"acl.cosi"`
	NebraskaCodebase      string `help:"Base URL returned by the fake Nebraska endpoint" default:"https://example.invalid/images/"`
	NebraskaSHA384        string `help:"SHA384 returned by the fake Nebraska endpoint" default:"111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111"`
	NodeName              string `help:"Node name served by the fake apiserver" default:"trident-node"`
	ExpectedInitialVolume string `help:"Expected active volume immediately after deployment" default:"volume-a"`
}
