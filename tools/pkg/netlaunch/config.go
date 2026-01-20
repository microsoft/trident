package netlaunch

import "tridenttools/pkg/bmc"

type NetCommonConfig struct {
	// Port to serve the HTTP server on.
	// If not specified (0), the server will be started on a random port.
	ListenPort uint16 `yaml:"listenPort,omitempty"`

	// File to write the logstream to.
	LogstreamFile string `yaml:"logstreamFile,omitempty"`

	// When set, netlaunch will write incoming tracing metrics data to the
	// specified file.
	TracestreamFile string `yaml:"tracestreamFile,omitempty"`

	// When set, netlaunch will serve files from the specified directory at
	// `/files/` on the HTTP server.
	ServeDirectory string `yaml:"serveDirectory,omitempty"`

	// Maximum numbers of failures to tolerate from incoming phonehome requests.
	// Useful for tests that manually induce failures.
	MaxPhonehomeFailures uint `yaml:"maxPhonehomeFailures,omitempty"`
}

type NetLaunchConfig struct {
	// Configuration common to both netlaunch and netlisten.
	NetCommonConfig `yaml:",inline"`

	// Configuration for the netlaunch reverse-connect proxy. When omitted,
	// netlaunch will not configure the in-image rcp-agent service, and will
	// default to legacy Host Config injection.
	Rcp *RcpConfiguration `yaml:"rcp,omitempty"`

	// Configuration about how to launch hosts, VM or baremetal.
	Netlaunch HostConnectionConfiguration `yaml:"netlaunch"`

	// Configuration for overriding ISO content.
	Iso IsoConfig `yaml:"iso,omitempty"`

	// Path to the ISO file that will be used for the netlaunch.
	IsoPath string `yaml:"isoPath,omitempty"`

	// Host Configuration file to inject into the ISO.
	//
	// If not specified, nothing will be injected.
	//
	// The file may contain the template string `NETLAUNCH_HOST_ADDRESS` which
	// will be replaced with the url of the root directory of the HTTP server.
	HostConfigFile string `yaml:"hostConfigFile,omitempty"`

	// When set, netlaunch will create a file at the specified path containing
	// the IP address of the remote host where Trident is running.
	RemoteAddressFile string `yaml:"remoteAddressFile,omitempty"`

	// When set, netlaunch will inject this certificate into the VM's EFI variables.
	// This is useful for self-signed certificates.
	CertificateFile string `yaml:"certificateFile,omitempty"`

	// Whether to enable secure boot for the VM.
	EnableSecureBoot bool `yaml:"enableSecureBoot,omitempty"`

	// Whether to wait for the VM to be provisioned before exiting.
	WaitForProvisioning bool `yaml:"waitForProvisioning,omitempty"`
}

// Configuration for netlaunch reverse-connect proxy.
type RcpConfiguration struct {
	// Run netlaunch in gRPC mode. When true, netlaunch will use the
	// reverse-connect proxy to communicate with Trident using gRPC. When false,
	// netlaunch will use the reverse-connect proxy to download the Host
	// Configuration file and start the legacy installation service.
	//
	// If omitted, defaults to false.
	GrpcMode bool `yaml:"grpcMode,omitempty"`

	// Port number to listen on for incoming connections from the
	// reverse-connect proxy.
	//
	// If omitted or set to 0, a random port will be chosen.
	ListenPort *uint16 `yaml:"listenPort,omitempty"`

	// An optional path to a local trident binary to copy into the remote host.
	// If not specified, no Trident binary will be copied.
	LocalTridentPath *string `yaml:"localTridentPath,omitempty"`

	// An optional path to a local osmodifier binary to copy into the remote host.
	// If not specified, no Osmodifier binary will be copied.
	LocalOsmodifierPath *string `yaml:"localOsmodifierPath,omitempty"`

	// Replace the execution for trident-install to use stream image instead of
	// the default installation method.
	UseStreamImage bool `yaml:"useStreamImage,omitempty"`
}

type HostConnectionConfiguration struct {
	// Configuration for physical/emulated BMCs.

	// IP address to announce to the BMC.
	//
	// If not specified, netlaunch will attempt to automatically detect the IP
	// address to announce by finding the local IP address that routes to the
	// BMC IP address.
	AnnounceIp *string `yaml:"announceIp,omitempty"`

	// Port number to announce to the BMC.
	//
	// If not specified, netlaunch will use the port number that the HTTP server
	// is listening on.
	AnnouncePort *uint16 `yaml:"announcePort,omitempty"`

	// Configuration to connect to the BMC.
	Bmc *bmc.Bmc `yaml:"bmc,omitempty"`

	// Configuration for local VMs.

	// UUID of the local libvirt VM to connect to.
	LocalVmUuid *string `yaml:"localVmUuid,omitempty"`

	// Path to the NVRAM file for the local libvirt VM.
	LocalVmNvRam *string `yaml:"localVmNvRam,omitempty"`
}

type IsoConfig struct {
	PreTridentScript     *string `yaml:"preTridentScript,omitempty"`
	ServiceOverride      *string `yaml:"serviceOverride,omitempty"`
	DirectStreamingImage *string `yaml:"directStreamingImage,omitempty"`
}

type NetListenConfig struct {
	// Configuration common to both netlaunch and netlisten.
	NetCommonConfig `yaml:",inline"`

	// Configuration for physical BMCs.
	Netlisten struct {
		Bmc *bmc.Bmc `yaml:"bmc,omitempty"`
	}
}

func (c *NetLaunchConfig) IsGrpcMode() bool {
	return c.Rcp != nil && c.Rcp.GrpcMode
}
