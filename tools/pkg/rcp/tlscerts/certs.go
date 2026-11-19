// This module defines the CertProvider interface used for mutual TLS
// authentication between the RCP client (netlaunch) and the RCP agent/proxy
// running on a freshly provisioned test target. Certificate material itself
// is generated fresh per test session at runtime -- see ephemeral.go -- not
// embedded into the binary at compile time.

package tlscerts

import (
	"crypto/tls"
)

// ServerSubjectAltName is the default SAN for the server certificate.
const ServerSubjectAltName = "reverseconnectproxy"

type CertProvider interface {
	LocalCert() (tls.Certificate, error)
	RemoteCertPEM() []byte
}
