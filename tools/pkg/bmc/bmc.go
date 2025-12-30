/*
Copyright © 2023 Microsoft Corporation
*/
package bmc

import (
	"context"
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

// ListenForSerialOutput sets up a serial over SSH session in a background
// goroutine and returns a handle to it.
func (b *Bmc) ListenForSerialOutput(ctx context.Context) (*serial.SerialOverSshSession, error) {
	serial, err := serial.NewSerialOverSshSession(ctx, serial.SerialOverSSHSettings{
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
