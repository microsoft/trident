package config

type TestConfig struct {
	ArtifactsDir       string `help:"Directory containing artifacts for the VM" default:"/tmp"`
	OutputPath         string `help:"Path to the output directory for logs and artifacts" default:"./output"`
	Verbose            bool   `help:"Enable verbose logging" default:"false"`
	RetryCount         int    `help:"Number of retry attempts for updates" default:"3"`
	Rollback           bool   `help:"Enable rollback testing" default:"false"`
	RollbackRetryCount int    `help:"Number of retry attempts for updates" default:"3"`
	UpdatePortA        int    `help:"Port for the first update server" default:"8000"`
	UpdatePortB        int    `help:"Port for the second update server" default:"8001"`
	ExpectedVolume     string `help:"Expected active volume after update" default:"volume-a"`
	ForceCleanup       bool   `help:"Force cleanup of VM when test finishes" default:"false"`
}
