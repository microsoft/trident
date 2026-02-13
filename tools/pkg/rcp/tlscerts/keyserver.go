package tlscerts

import (
	"crypto/tls"
	_ "embed"
)

//go:embed server.key
var serverKey []byte

// ServerCertProvider provides certificates for the TLS server.
var ServerCertProvider CertProvider = serverCertProviderImpl{}

type serverCertProviderImpl struct{}

func (p serverCertProviderImpl) LocalCert() (tls.Certificate, error) {
	return tls.X509KeyPair(serverCert, serverKey)
}

func (p serverCertProviderImpl) RemoteCertPEM() []byte {
	return clientCert
}
