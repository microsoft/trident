package agent

import (
	"crypto/tls"
	"encoding/base64"
	"fmt"
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
	ClientCert string `yaml:"certData,omitempty" mapstructure:"certData"`
	ClientKey  string `yaml:"keyData,omitempty" mapstructure:"keyData"`
	ServerCert string `yaml:"serverCert,omitempty" mapstructure:"serverCert"`
}

// LocalCert implements the CertProvider interface.
func (d *RcpTlsClientData) LocalCert() (tls.Certificate, error) {
	clientCert, err := base64.StdEncoding.DecodeString(d.ClientCert)
	if err != nil {
		return tls.Certificate{}, fmt.Errorf("failed to decode client certificate: %w", err)
	}

	clientKey, err := base64.StdEncoding.DecodeString(d.ClientKey)
	if err != nil {
		return tls.Certificate{}, fmt.Errorf("failed to decode client key: %w", err)
	}

	return tls.X509KeyPair(clientCert, clientKey)
}

// RemoteCertPEM implements the CertProvider interface.
func (d *RcpTlsClientData) RemoteCertPEM() []byte {
	cert, err := base64.StdEncoding.DecodeString(d.ServerCert)
	if err != nil {
		return nil
	}

	return cert
}
