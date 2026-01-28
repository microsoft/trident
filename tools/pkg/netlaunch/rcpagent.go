package netlaunch

import (
	"crypto/tls"
	"os"
)

// RcpAgentConfiguration holds the configuration for the RCP agent.

type RcpAgentConfiguration struct {
	ClientAddress   string              `yaml:"clientAddress,omitempty" mapstructure:"clientAddress"`
	ServerAddress   string              `yaml:"serverAddress,omitempty" mapstructure:"serverAddress"`
	AdditionalFiles []RcpAdditionalFile `yaml:"additionalFiles,omitempty" mapstructure:"additionalFiles"`
	RcpClientTls    RcpTlsClientData    `yaml:"rcpClientTls,omitempty" mapstructure:"rcpClientTls"`
}

type RcpAdditionalFile struct {
	DownloadUrl string      `yaml:"downloadUrl,omitempty" mapstructure:"downloadUrl"`
	Destination string      `yaml:"destination,omitempty" mapstructure:"destination"`
	Mode        os.FileMode `yaml:"mode,omitempty" mapstructure:"mode"`
}

type RcpTlsClientData struct {
	ClientCert []byte `yaml:"certData,omitempty" mapstructure:"certData"`
	ClientKey  []byte `yaml:"keyData,omitempty" mapstructure:"keyData"`
	ServerCert []byte `yaml:"serverCert,omitempty" mapstructure:"serverCert"`
}

// LocalCert implements the CertProvider interface.
func (d *RcpTlsClientData) LocalCert() (tls.Certificate, error) {
	return tls.X509KeyPair(d.ClientCert, d.ClientKey)
}

// RemoteCertPEM implements the CertProvider interface.
func (d *RcpTlsClientData) RemoteCertPEM() []byte {
	return d.ServerCert
}
