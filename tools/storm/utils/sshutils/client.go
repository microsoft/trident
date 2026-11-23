package sshutils

import (
	"context"
	"fmt"
	"net"
	"strconv"
	"time"

	stormssh "github.com/microsoft/storm/utils/ssh"
	"golang.org/x/crypto/ssh"
)

type SshClientConfig struct {
	User       string
	Host       string
	Port       uint16
	PrivateKey []byte
	Timeout    time.Duration
}

func (c *SshClientConfig) PortOrDefault() uint16 {
	if c.Port == 0 {
		return 22
	}
	return c.Port
}

func (c *SshClientConfig) FullHost() string {
	return net.JoinHostPort(c.Host, strconv.Itoa(int(c.PortOrDefault())))
}

func (c *SshClientConfig) facilitiesConfig() (stormssh.Config, error) {
	signer, err := ssh.ParsePrivateKey(c.PrivateKey)
	if err != nil {
		return stormssh.Config{}, fmt.Errorf("failed to parse SSH key: %w", err)
	}
	return stormssh.Config{
		Addr:             c.FullHost(),
		User:             c.User,
		Auth:             signer,
		HostKey:          stormssh.InsecureHostKey(),
		DialTimeout:      c.Timeout,
		RetryAuthFailure: true, // A booting guest may not have installed its authorized key yet.
	}, nil
}

func CreateSshClient(ctx context.Context, config SshClientConfig) (*stormssh.Client, error) {
	cfg, err := config.facilitiesConfig()
	if err != nil {
		return nil, err
	}
	return stormssh.Dial(ctx, cfg)
}

func CreateSshClientWithRedial(ctx context.Context, backoff time.Duration, config SshClientConfig) (*stormssh.Client, error) {
	cfg, err := config.facilitiesConfig()
	if err != nil {
		return nil, err
	}
	// Keep individual attempts short while the caller bounds the overall wait.
	cfg.DialTimeout = 5 * time.Second
	return stormssh.WaitReady(ctx, cfg, backoff)
}

func RunCommand(client *stormssh.Client, command string) (*SshCmdOutput, error) {
	if client == nil {
		return nil, fmt.Errorf("SSH client is nil")
	}
	result, err := client.Run(context.Background(), command)
	return &SshCmdOutput{
		Stdout: string(result.Stdout),
		Stderr: string(result.Stderr),
		Status: result.ExitCode,
	}, err
}

func CommandOutput(client *stormssh.Client, command string) (string, error) {
	output, err := RunCommand(client, command)
	if err != nil {
		return "", err
	}
	if err := output.Check(); err != nil {
		return "", fmt.Errorf("command failed: %s: %w", output.Report(), err)
	}
	return output.Stdout, nil
}

func WaitForDisconnect(ctx context.Context, client *stormssh.Client) error {
	if client == nil {
		return fmt.Errorf("SSH client is nil")
	}
	return client.WaitDisconnected(ctx)
}
