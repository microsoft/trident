package templates

import (
	_ "embed"
	"net"
)

//go:embed network.xml
var networkXMLTemplateStr string

type NetworkTemplateParams struct {
	Name         string
	NatInterface string
	Network      string
	Address      string
	DHCPStart    net.IP
	DHCPEnd      net.IP
	Hosts        []NetworkHost
}

type NetworkHost struct {
	Name string
	MAC  string
	IP   net.IP
}

func NetworkXML(params NetworkTemplateParams) (string, error) {
	return renderTemplate("network", networkXMLTemplateStr, params)

}
