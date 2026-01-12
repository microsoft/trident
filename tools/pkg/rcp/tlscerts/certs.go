// This module contains self-signed TLS certificates for use by the RCP
// client and proxy for mutual TLS authentication during testing.

package tlscerts

import (
	"crypto/tls"
	_ "embed"
)

//go:generate go run generate.go generate --san reverseconnectproxy

// ServerSubjectAltName is the default SAN for the server certificate.
const ServerSubjectAltName = "reverseconnectproxy"

type CertProvider interface {
	LocalCert() (tls.Certificate, error)
	RemoteCertPEM() []byte
}

//go:embed server.crt
var serverCert []byte

//go:embed client.crt
var clientCert []byte
