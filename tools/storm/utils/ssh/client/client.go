package client

import (
	"fmt"
	"io"
	"os"
	"time"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	stormsshconfig "tridenttools/storm/utils/ssh/config"
)

func OpenSshClient(settings stormsshconfig.SshCliSettings) (*ssh.Client, error) {
	private_key, err := os.ReadFile(settings.PrivateKeyPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read SSH key file '%s': %w", settings.PrivateKeyPath, err)
	}

	signer, err := ssh.ParsePrivateKey(private_key)
	if err != nil {
		return nil, fmt.Errorf("failed to parse SSH key: %w", err)
	}

	clientConfig := &ssh.ClientConfig{
		User: settings.User,
		Auth: []ssh.AuthMethod{
			ssh.PublicKeys(signer),
		},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
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

	out := &stormsshconfig.SshCmdOutput{
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
