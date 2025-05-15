package utils

import (
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/pkg/sftp"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

const (
	AZL3_SFTP_SERVER_PATH = "/usr/libexec/sftp-server"
	AZL3_SFTP_SERVER_CMD  = "sudo -n " + AZL3_SFTP_SERVER_PATH
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

func OpenSshClient(settings SshCliSettings) (*ssh.Client, error) {
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
