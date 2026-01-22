//go:build tls_client

package tlscerts

import (
	"crypto/tls"
	_ "embed"
)

//go:embed client.key
var clientKey []byte

// ClientCertProvider provides certificates for the TLS client.
var ClientCertProvider CertProvider = clientCertProviderImpl{}

type clientCertProviderImpl struct{}

func (p clientCertProviderImpl) LocalCert() (tls.Certificate, error) {
	return tls.X509KeyPair(clientCert, clientKey)
}

func (p clientCertProviderImpl) RemoteCertPEM() []byte {
	return serverCert
}
