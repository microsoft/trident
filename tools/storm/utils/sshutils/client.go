package sshutils

import (
	"context"
	"fmt"
	"io"
	"net"
	"time"
	"tridenttools/storm/utils/retry"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

type SshClientConfig struct {
	// Username for SSH connection.
	User string
	// Host to connect to.
	Host string
	// Port to connect to. If not specified, defaults to 22.
	Port uint16
	// Private key for authentication in PEM format.
	PrivateKey []byte
	// Connection timeout. If zero, no timeout is set.
	Timeout time.Duration
}

// PortOrDefault returns the configured port or the default SSH port (22) if not set.
func (c *SshClientConfig) PortOrDefault() uint16 {
	if c.Port == 0 {
		return 22
	}
	return c.Port
}

// FullHost returns the full host string in the format "host:port".
func (c *SshClientConfig) FullHost() string {
	return fmt.Sprintf("%s:%d", c.Host, c.PortOrDefault())
}

// CreateSshClient creates and returns an SSH client based on the provided
// configuration. It will attempt a single connection using the provided context
// for timeout and cancellation.
func CreateSshClient(ctx context.Context, config SshClientConfig) (*ssh.Client, error) {
	signer, err := ssh.ParsePrivateKey(config.PrivateKey)
	if err != nil {
		return nil, fmt.Errorf("failed to parse SSH key: %w", err)
	}

	clientConfig := &ssh.ClientConfig{
		User: config.User,
		Auth: []ssh.AuthMethod{
			ssh.PublicKeys(signer),
		},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(), // CodeQL [SM03565] This is test code, not production code
		Timeout:         config.Timeout,
	}

	port := config.Port
	if port == 0 {
		port = 22
	}

	host := fmt.Sprintf("%s:%d", config.Host, port)

	// Create a net.Dialer manually so that we may use the DialContext method.
	// This allows us to respect the provided context for timeouts and
	// cancellations instead of expecting a timeout parameter. The following
	// code is the same as ssh.Dial, except we use DialContext.
	d := net.Dialer{}
	conn, err := d.DialContext(ctx, "tcp", host)
	if err != nil {
		return nil, fmt.Errorf("failed to dial SSH server '%s': %w", host, err)
	}

	c, chans, reqs, err := ssh.NewClientConn(conn, host, clientConfig)
	if err != nil {
		return nil, fmt.Errorf("failed to open SSH connection to '%s': %w", host, err)
	}

	return ssh.NewClient(c, chans, reqs), nil
}

// CreateSshClientWithRedial creates and returns an SSH client based on the
// provided configuration. It will keep attempting to connect until the context
// is cancelled, waiting for the specified backoff duration between attempts.
func CreateSshClientWithRedial(ctx context.Context, backoff time.Duration, config SshClientConfig) (*ssh.Client, error) {
	return retry.RetryContext(ctx, backoff, func(ctx context.Context, attempt int) (*ssh.Client, error) {
		// Create a short timeout context for each individual connection attempt.
		short_ctx, cancel := context.WithTimeout(ctx, time.Duration(5)*time.Second)
		defer cancel()

		return CreateSshClient(short_ctx, config)
	})
}

func RunCommand(client *ssh.Client, command string) (*SshCmdOutput, error) {
	if client == nil {
		return nil, fmt.Errorf("SSH client is nil")
	}

	session, err := client.NewSession()
	if err != nil {
		return nil, fmt.Errorf("failed to create SSH session: %w", err)
	}
	defer session.Close()

	stdout, err := session.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stdout pipe: %w", err)
	}

	stderr, err := session.StderrPipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stderr pipe: %w", err)
	}

	err = session.Start(command)
	if err != nil {
		return nil, fmt.Errorf("failed to start command: %w", err)
	}

	output, err := io.ReadAll(stdout)
	if err != nil {
		return nil, fmt.Errorf("failed to read stdout: %w", err)
	}

	errOutput, err := io.ReadAll(stderr)
	if err != nil {
		return nil, fmt.Errorf("failed to read stderr: %w", err)
	}

	out := &SshCmdOutput{
		Stdout: string(output),
		Stderr: string(errOutput),
		Status: 0,
	}

	err = session.Wait()
	if err != nil {
		if exitErr, ok := err.(*ssh.ExitError); ok {
			// If we failed with an exit error, capture the exit status and set
			// it in the output struct. This is useful for checking the status
			// of the command after it has completed.
			out.Status = exitErr.ExitStatus()
		} else {
			out.Status = -1
			return out, err
		}
	}

	return out, nil
}

func CommandOutput(client *ssh.Client, command string) (string, error) {
	logrus.WithField("command", command).Debug("Executing command")
	output, err := RunCommand(client, command)
	if err != nil {
		logrus.Errorf("Failed to run command: %s", err)
		return "", fmt.Errorf("failed to run command: %w", err)
	}

	if err := output.Check(); err != nil {
		logrus.Errorf("Command failed: %s", output.Report())
		return "", fmt.Errorf("command failed: %w", err)
	}

	return output.Stdout, nil
}
