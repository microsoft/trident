package client

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"time"
	"tridenttools/storm/utils/sshutils"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	stormsshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/ssh/sftp"
)

func OpenSshClient(settings stormsshconfig.SshCliSettings) (*ssh.Client, error) {
	privateKey, err := os.ReadFile(settings.PrivateKeyPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read SSH key file '%s': %w", settings.PrivateKeyPath, err)
	}

	signer, err := ssh.ParsePrivateKey(privateKey)
	if err != nil {
		return nil, fmt.Errorf("failed to parse SSH key: %w", err)
	}

	clientConfig := &ssh.ClientConfig{
		User: settings.User,
		Auth: []ssh.AuthMethod{
			ssh.PublicKeys(signer),
		},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(), // CodeQL [SM03565] This is test code, not production code
		Timeout:         time.Second * time.Duration(settings.Timeout),
	}

	host := fmt.Sprintf("%s:%d", settings.Host, settings.Port)

	client, err := ssh.Dial("tcp", host, clientConfig)
	if err != nil {
		return nil, fmt.Errorf("failed to dial SSH server '%s': %w", host, err)
	}

	return client, nil
}

func RunCommand(client *ssh.Client, command string) (*stormsshconfig.SshCmdOutput, error) {
	return RunCommandContext(context.Background(), client, command)
}

func OpenSshClientContext(ctx context.Context, settings stormsshconfig.SshCliSettings) (*ssh.Client, error) {
	cfg, err := settings.IntoClientConfig()
	if err != nil {
		return nil, err
	}
	return sshutils.CreateSshClient(ctx, *cfg)
}

// RunCommandContext preserves separate output streams and cancels by closing the connection.
func RunCommandContext(ctx context.Context, client *ssh.Client, command string) (*stormsshconfig.SshCmdOutput, error) {
	if client == nil {
		return nil, fmt.Errorf("SSH client is nil")
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	stop := context.AfterFunc(ctx, func() { _ = client.Close() })
	defer stop()

	session, err := client.NewSession()
	if err != nil {
		return nil, fmt.Errorf("failed to create SSH session: %w", err)
	}
	defer session.Close()

	var stdout, stderr bytes.Buffer
	session.Stdout, session.Stderr = &stdout, &stderr
	err = session.Run(command)
	out := &stormsshconfig.SshCmdOutput{
		Stdout: stdout.String(),
		Stderr: stderr.String(),
		Status: 0,
	}
	if ctx.Err() != nil {
		out.Status = -1
		return out, ctx.Err()
	}
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

// CopyRemoteFileToLocal copies a file from a remote host to the local system via SFTP.
func CopyRemoteFileToLocal(client *ssh.Client, remotePath string, localFilePath string) error {
	sftpClient, err := sftp.NewSftpSudoClient(client)
	if err != nil {
		return fmt.Errorf("failed to create SFTP client: %w", err)
	}
	defer sftpClient.Close()

	remoteFile, err := sftpClient.Open(remotePath)
	if err != nil {
		return fmt.Errorf("failed to open remote file %s: %w", remotePath, err)
	}
	defer remoteFile.Close()

	localFile, err := os.Create(localFilePath)
	if err != nil {
		return fmt.Errorf("failed to create local file: %w", err)
	}
	defer localFile.Close()

	if _, err := io.Copy(localFile, remoteFile); err != nil {
		return fmt.Errorf("failed to copy file to local system: %w", err)
	}

	return nil
}
