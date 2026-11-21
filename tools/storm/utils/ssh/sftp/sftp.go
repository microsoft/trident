package sftp

import (
	"errors"
	"fmt"
	"io"
	"os"

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

// Downloads a remote file via SFTP with sudo privileges.
func DownloadRemoteFile(client *ssh.Client, remotePath string, localPath string) (string, error) {
	var localFile *os.File
	var err error
	if localPath == "" {
		localFile, err = os.CreateTemp("", "sftp-*")
		if err != nil {
			return "", fmt.Errorf("failed to create local tmp file: %w", err)
		}
		localPath = localFile.Name()
	} else {
		localFile, err = os.Create(localPath)
		if err != nil {
			return "", fmt.Errorf("failed to create local file (%s): %w", localPath, err)
		}
	}
	defer localFile.Close()

	sftpClient, err := NewSftpSudoClient(client)
	if err != nil {
		return "", fmt.Errorf("failed to create SudoSFTP client: %w", err)
	}
	defer sftpClient.Close()

	remoteDatastoreFile, err := sftpClient.Open(remotePath)
	if err != nil {
		return "", fmt.Errorf("failed to open remote file (%s): %w", remotePath, err)
	}
	defer remoteDatastoreFile.Close()

	_, err = remoteDatastoreFile.WriteTo(localFile)
	if err != nil {
		return "", fmt.Errorf("failed to copy remote file to local: %w", err)
	}

	return localPath, nil
}

// UploadLocalFile copies a local file through the same privileged SFTP transport as downloads.
func UploadLocalFile(client *ssh.Client, localPath, remotePath string) error {
	source, err := os.Open(localPath)
	if err != nil {
		return err
	}
	defer source.Close()
	remote, err := NewSftpSudoClient(client)
	if err != nil {
		return err
	}
	defer remote.Close()
	target, err := remote.OpenFile(remotePath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC)
	if err != nil {
		return err
	}
	if err := target.Chmod(0600); err != nil {
		return errors.Join(err, target.Close())
	}
	_, copyErr := io.Copy(target, source)
	return errors.Join(copyErr, target.Close())
}
