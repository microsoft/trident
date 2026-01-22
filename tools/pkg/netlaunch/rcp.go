package netlaunch

// RcpAgentConfiguration holds the configuration for the RCP agent.
//
// Note: because of how kong works, the YAML field names must match the kong name.
type RcpAgentConfiguration struct {
	ClientAddress string `yaml:"clientAddress,omitempty" short:"c" name:"clientAddress" aliases:"client-address" help:"Address of the rcp-client to connect to"`
	ServerAddress string `yaml:"serverAddress,omitempty" short:"s" name:"serverAddress" aliases:"server-address" help:"Address of the server to connect to" default:"${defaultServerAddress}"`

	// Non-CLI params that can be obtained from the YAML config file

	// An optional URL to download Trident from.
	TridentDownloadUrl string `yaml:"tridentDownloadUrl,omitempty"`
}
