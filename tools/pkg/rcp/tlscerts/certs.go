package tlscerts

import (
	_ "embed"
)

//go:generate go run generate.go generate --san reverseconnectproxy

// ServerSubjectAltName is the default SAN for the server certificate.
const ServerSubjectAltName = "reverseconnectproxy"

//go:embed server.crt
var serverCert []byte

func GetServerCertPEM() []byte {
	return serverCert
}

//go:embed client.crt
var clientCert []byte

func GetClientCertPEM() []byte {
	return clientCert
}
