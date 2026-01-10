//go:build tls_server

package tlscerts

import (
	"crypto/tls"
	_ "embed"
)

//go:embed server.key
var serverKey []byte

func GetServerX509Cert() (tls.Certificate, error) {
	return tls.X509KeyPair(serverCert, serverKey)
}
