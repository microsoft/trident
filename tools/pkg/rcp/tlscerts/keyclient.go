//go:build tls_client

package tlscerts

import (
	"crypto/tls"
	_ "embed"
)

//go:embed client.key
var clientKey []byte

func GetClientX509Cert() (tls.Certificate, error) {
	return tls.X509KeyPair(clientCert, clientKey)
}
