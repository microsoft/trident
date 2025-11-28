/*
Copyright © 2023 Microsoft Corporation
*/
package bmc

import (
	"tridenttools/pkg/serial"

	log "github.com/sirupsen/logrus"
)

type Bmc struct {
	Ip            string
	Port          *string
	Username      string
	Password      string
	SerialOverSsh *struct {
		SshPort uint16
		ComPort string
		Output  string
	}
}

func (b *Bmc) ListenForSerialOutput() (*serial.SerialOverSshSession, error) {
	serial, err := serial.NewSerialOverSshSession(serial.SerialOverSSHSettings{
		Host:     b.Ip,
		Port:     b.SerialOverSsh.SshPort,
		Username: b.Username,
		Password: b.Password,
		ComPort:  b.SerialOverSsh.ComPort,
		Output:   b.SerialOverSsh.Output,
	})
	if err != nil {
		log.WithError(err).Fatalf("Failed to open serial over SSH session")
		return nil, err
	}
	return serial, nil
}
