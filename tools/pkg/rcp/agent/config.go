package agent

import (
	"crypto/tls"
	"encoding/base64"
	"fmt"
	"os"

	log "github.com/sirupsen/logrus"
)

// RcpAgentConfiguration holds the configuration for the RCP agent.

type RcpAgentConfiguration struct {
	ClientAddress        string                `yaml:"clientAddress,omitempty" mapstructure:"clientAddress"`
	ServerConnectionType string                `yaml:"serverConnectionType,omitempty" mapstructure:"serverConnectionType"`
	ServerAddress        string                `yaml:"serverAddress,omitempty" mapstructure:"serverAddress"`
	AdditionalFiles      []RcpAdditionalFile   `yaml:"additionalFiles,omitempty" mapstructure:"additionalFiles"`
	RcpClientTls         RcpTlsClientData      `yaml:"rcpClientTls,omitempty" mapstructure:"rcpClientTls"`
	Services             ServicesConfiguration `yaml:"services,omitempty" mapstructure:"services"`
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

type ServicesConfiguration struct {
	Start []string `yaml:"start,omitempty" mapstructure:"start"`
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
		log.Errorf("failed to decode server certificate: %v", err)
		return nil
	}

	return cert
}
