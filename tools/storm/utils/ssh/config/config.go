package config

import (
	"fmt"
	"os"
	"strings"
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

type SshCmdOutput struct {
	Stdout string
	Stderr string
	Status int
}

// Returns an error if the command finished with a non-zero status.
func (o *SshCmdOutput) Check() error {
	if o.Status != 0 {
		return fmt.Errorf("command failed with status %d", o.Status)
	}

	return nil
}

func (o *SshCmdOutput) Report() string {
	if o == nil {
		return "<nil>"
	}

	var stringBuilder strings.Builder
	stringBuilder.WriteString(fmt.Sprintf("status: %d", o.Status))

	if o.Stdout != "" {
		stringBuilder.WriteString(fmt.Sprintf("; stdout:\n%s\nstderr:", o.Stdout))
	} else {
		stringBuilder.WriteString("; stdout: <empty>; stderr:")
	}

	if o.Stderr != "" {
		stringBuilder.WriteString(fmt.Sprintf("\n%s", o.Stderr))
	} else {
		stringBuilder.WriteString(" <empty>")
	}

	return stringBuilder.String()
}
