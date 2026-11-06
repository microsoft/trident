// Ephemeral, in-memory TLS certificate generation for RCP sessions.
//
// Certificates are generated fresh for each netlaunch invocation rather than
// embedded into the binary at compile time: a compile-time embedded pair is
// shared by every deployment built from the same binary, so anyone who
// obtains a released netlaunch/rcp-agent build also obtains valid
// credentials for both sides of every test session using that build. An
// ephemeral, per-session pair generated here has no value outside the
// single test run it was created for, and nothing is ever written to disk.

package tlscerts

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"math/big"
	"time"
)

// inMemoryCertProvider implements CertProvider using certificate material
// held only in process memory for the lifetime of a single netlaunch
// invocation.
type inMemoryCertProvider struct {
	local  tls.Certificate
	remote []byte
}

func (p *inMemoryCertProvider) LocalCert() (tls.Certificate, error) {
	return p.local, nil
}

func (p *inMemoryCertProvider) RemoteCertPEM() []byte {
	return p.remote
}

// generateSelfSignedCert creates a fresh self-signed ECDSA certificate and
// key for the given Subject Alternative Name, valid only for the lifetime of
// a single test session. NotBefore is backdated slightly to tolerate clock
// skew between the orchestrator and a freshly provisioned target.
func generateSelfSignedCert(subjectAltName string) (certPEM, keyPEM []byte, err error) {
	priv, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to generate key: %w", err)
	}

	serialNumber, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	if err != nil {
		return nil, nil, fmt.Errorf("failed to generate serial number: %w", err)
	}

	template := x509.Certificate{
		SerialNumber:          serialNumber,
		Subject:               pkix.Name{CommonName: subjectAltName},
		DNSNames:              []string{subjectAltName},
		NotBefore:             time.Now().Add(-5 * time.Minute),
		NotAfter:              time.Now().Add(24 * time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth, x509.ExtKeyUsageClientAuth},
		BasicConstraintsValid: true,
	}

	certDER, err := x509.CreateCertificate(rand.Reader, &template, &template, &priv.PublicKey, priv)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to create certificate: %w", err)
	}
	certPEM = pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: certDER})

	keyDER, err := x509.MarshalECPrivateKey(priv)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to marshal private key: %w", err)
	}
	keyPEM = pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: keyDER})

	return certPEM, keyPEM, nil
}

// NewEphemeralSession generates a fresh client/server certificate pair for a
// single RCP session between netlaunch and rcp-agent. Both halves are
// generated together so the caller can configure its own listener
// (serverProvider) and hand the matching client half to the freshly
// provisioned target in the same call. Nothing generated here is written to
// disk or reused across invocations.
func NewEphemeralSession() (serverProvider CertProvider, clientCertPEM, clientKeyPEM, serverCertPEM []byte, err error) {
	serverCertPEM, serverKeyPEM, err := generateSelfSignedCert(ServerSubjectAltName)
	if err != nil {
		return nil, nil, nil, nil, fmt.Errorf("failed to generate server cert: %w", err)
	}

	clientCertPEM, clientKeyPEM, err = generateSelfSignedCert("rcp-client")
	if err != nil {
		return nil, nil, nil, nil, fmt.Errorf("failed to generate client cert: %w", err)
	}

	localCert, err := tls.X509KeyPair(serverCertPEM, serverKeyPEM)
	if err != nil {
		return nil, nil, nil, nil, fmt.Errorf("failed to load server keypair: %w", err)
	}

	provider := &inMemoryCertProvider{local: localCert, remote: clientCertPEM}
	return provider, clientCertPEM, clientKeyPEM, serverCertPEM, nil
}
