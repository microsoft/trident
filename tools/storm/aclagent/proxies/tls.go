package proxies

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
	"net"
	"time"
)

// GenerateEphemeralTLSCert creates a self-signed, in-memory TLS certificate
// (and its matching PEM-encoded certificate, for callers that need to hand
// it to trident-acl-agent/tridentd as a trusted CA) valid for host, which
// may be an IP address or a DNS name. Nothing is written to disk here: this
// is deliberately test-only throwaway key material, generated fresh for
// every storm run, per policy against committing certs to the repo.
//
// The returned certificate is self-signed (its own issuer), so callers that
// need something else to trust it (trident-acl-agent's Nebraska client, or
// the VM's system trust store for tridentd's image download) must install
// certPEM itself as a trusted root, not a CA that issued it.
func GenerateEphemeralTLSCert(host string) (cert tls.Certificate, certPEM []byte, err error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return tls.Certificate{}, nil, fmt.Errorf("failed to generate ephemeral TLS key: %w", err)
	}

	serialLimit := new(big.Int).Lsh(big.NewInt(1), 128)
	serial, err := rand.Int(rand.Reader, serialLimit)
	if err != nil {
		return tls.Certificate{}, nil, fmt.Errorf("failed to generate certificate serial number: %w", err)
	}

	template := &x509.Certificate{
		SerialNumber:          serial,
		Subject:               pkix.Name{CommonName: host},
		NotBefore:             time.Now().Add(-5 * time.Minute),
		NotAfter:              time.Now().Add(24 * time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
	}
	if ip := net.ParseIP(host); ip != nil {
		template.IPAddresses = []net.IP{ip}
	} else {
		template.DNSNames = []string{host}
	}

	derBytes, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		return tls.Certificate{}, nil, fmt.Errorf("failed to create ephemeral TLS certificate: %w", err)
	}

	certPEM = pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: derBytes})
	keyDER, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		return tls.Certificate{}, nil, fmt.Errorf("failed to marshal ephemeral TLS key: %w", err)
	}
	keyPEM := pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: keyDER})

	cert, err = tls.X509KeyPair(certPEM, keyPEM)
	if err != nil {
		return tls.Certificate{}, nil, fmt.Errorf("failed to load ephemeral TLS key pair: %w", err)
	}
	return cert, certPEM, nil
}
