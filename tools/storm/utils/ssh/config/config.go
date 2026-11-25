package config

import (
	"fmt"
	"os"
	"time"

	"tridenttools/storm/utils/sshutils"
)

type SshCliSettings struct {
	PrivateKeyPath string `arg:"" help:"Path to the SSH key file" type:"existingfile"`
	Host           string `arg:"" help:"Host to check SSH connection"`
	User           string `arg:"" help:"User to use for SSH connection"`
	Port           uint16 `short:"p" help:"Port to connect to" default:"22"`
	Timeout        int    `short:"t" help:"Timeout in seconds for the first SSH connection" default:"600"`
}

func (s *SshCliSettings) TimeoutDuration() time.Duration {
	return time.Second * time.Duration(s.Timeout)
}

func (s *SshCliSettings) FullHost() string {
	return fmt.Sprintf("%s:%d", s.Host, s.Port)
}

func (s *SshCliSettings) IntoClientConfig() (*sshutils.SshClientConfig, error) {
	privateKey, err := os.ReadFile(s.PrivateKeyPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read SSH key file '%s': %w", s.PrivateKeyPath, err)
	}

	return &sshutils.SshClientConfig{
		Host:       s.Host,
		Port:       s.Port,
		User:       s.User,
		PrivateKey: privateKey,
		Timeout:    s.TimeoutDuration(),
	}, nil
}

type SshCmdOutput = sshutils.SshCmdOutput
