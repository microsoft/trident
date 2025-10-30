package sftp

import (
	"fmt"

	"github.com/pkg/sftp"
	"golang.org/x/crypto/ssh"
)

const (
	AZL3_SFTP_SERVER_PATH = "/usr/libexec/sftp-server"
	AZL3_SFTP_SERVER_CMD  = "sudo -n " + AZL3_SFTP_SERVER_PATH
)

type SftpSudoClient struct {
	*sftp.Client
	inner *ssh.Session
}

func (c *SftpSudoClient) Close() error {
	if c.Client != nil {
		if err := c.Client.Close(); err != nil {
			return fmt.Errorf("failed to close SFTP client: %w", err)
		}
	}

	if c.inner != nil {
		if err := c.inner.Close(); err != nil {
			return fmt.Errorf("failed to close SSH session: %w", err)
		}
	}

	return nil
}

// Creates a new SFTP client with sudo privileges over SSH.
// It assumes the SSH user has passwordless sudo access.
func NewSftpSudoClient(client *ssh.Client, opts ...sftp.ClientOption) (*SftpSudoClient, error) {
	if client == nil {
		return nil, fmt.Errorf("SSH client is nil")
	}

	session, err := client.NewSession()
	if err != nil {
		return nil, fmt.Errorf("failed to create SSH session: %w", err)
	}

	ok, err := session.SendRequest("exec", true, ssh.Marshal(struct{ Command string }{AZL3_SFTP_SERVER_CMD}))
	if err == nil && !ok {
		err = fmt.Errorf("sftp: command %v failed", AZL3_SFTP_SERVER_CMD)
	}
	if err != nil {
		return nil, err
	}

	stdin, err := session.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stdin pipe: %w", err)
	}

	stdout, err := session.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("failed to create stdout pipe: %w", err)
	}

	sftpClient, err := sftp.NewClientPipe(stdout, stdin, opts...)
	if err != nil {
		return nil, fmt.Errorf("failed to create SFTP client: %w", err)
	}

	return &SftpSudoClient{
		Client: sftpClient,
		inner:  session,
	}, nil
}
