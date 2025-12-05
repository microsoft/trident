package keys

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"encoding/pem"
	"fmt"

	"golang.org/x/crypto/ssh"
)

// GenerateRsaKeyPair generates an RSA key pair of the specified bit size.
func GenerateRsaKeyPair(bitSize int) (privateKey []byte, publicKey []byte, err error) {
	key, err := rsa.GenerateKey(rand.Reader, bitSize)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to generate RSA key: %w", err)
	}

	pub := key.Public()

	// Encode private key to PKCS#1 ASN.1 PEM.
	keyPEM := pem.EncodeToMemory(
		&pem.Block{
			Type:  "RSA PRIVATE KEY",
			Bytes: x509.MarshalPKCS1PrivateKey(key),
		},
	)

	pubKeySsh, err := ssh.NewPublicKey(pub)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to encode SSH public key: %w", err)
	}

	return keyPEM, ssh.MarshalAuthorizedKey(pubKeySsh), nil
}
