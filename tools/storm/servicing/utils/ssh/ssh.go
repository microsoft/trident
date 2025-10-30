package ssh

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"
	"tridenttools/storm/servicing/utils/config"
	"tridenttools/storm/utils/retry"
	sshclient "tridenttools/storm/utils/ssh/client"
	sshconfig "tridenttools/storm/utils/ssh/config"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

func SshCommandWithRetries(cfg config.VMConfig, vmIP, command string, connectionRetryCount int, commandRetryCount int) (string, error) {
	return innerSshCommand(cfg, vmIP, command, false, connectionRetryCount, commandRetryCount)
}

func SshCommand(cfg config.VMConfig, vmIP, command string) (string, error) {
	return innerSshCommand(cfg, vmIP, command, false, 0, 0)
}

func SshCommandCombinedOutput(cfg config.VMConfig, vmIP, command string) (string, error) {
	return innerSshCommand(cfg, vmIP, command, true, 0, 0)
}

func StartSshProxyPortAndWait(ctx context.Context, port int, vmIP string, sshUser string, sshKeyPath string, startedChannel chan bool) error {
	cmd := exec.CommandContext(ctx,
		"ssh",
		"-R", fmt.Sprintf("%d:localhost:%d", port, port),
		"-N",
		"-o", "BatchMode=yes",
		"-o", "ConnectTimeout=10",
		"-o", "ServerAliveCountMax=3",
		"-o", "ServerAliveInterval=5",
		"-o", "StrictHostKeyChecking=no",
		"-o", "UserKnownHostsFile=/dev/null",
		"-i", sshKeyPath,
		fmt.Sprintf("%s@%s", sshUser, vmIP),
	)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	logrus.Tracef("Starting SSH proxy for port %d to VM %s with user %s", port, vmIP, sshUser)
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("failed to start SSH proxy for port %d: %w", port, err)
	}
	// Signal that the SSH proxy has started
	startedChannel <- true
	// Wait for the command to finish
	if err := cmd.Wait(); err != nil {
		return fmt.Errorf("SSH proxy for port %d failed: %w", port, err)
	}
	logrus.Tracef("SSH proxy for port %d exited", port)

	return nil
}

func ScpDownloadFile(cfg config.VMConfig, vmIP, src, dest string) error {
	args := []string{
		"-i", cfg.SshPrivateKeyPath,
		"-r",
		"-o", "StrictHostKeyChecking=no",
		"-o", "UserKnownHostsFile=/dev/null",
		fmt.Sprintf("%s@%s:%s", cfg.User, vmIP, src),
		dest,
	}
	logrus.Tracef("Running scp download with args: %v", args)
	cmd := exec.Command("scp", args...)
	return cmd.Run()
}

func ScpUploadFile(cfg config.VMConfig, vmIP, src, dest string) error {
	args := []string{
		"-i", cfg.SshPrivateKeyPath,
		"-r",
		"-o", "StrictHostKeyChecking=no",
		"-o", "UserKnownHostsFile=/dev/null",
		src,
		fmt.Sprintf("%s@%s:%s", cfg.User, vmIP, dest),
	}
	logrus.Tracef("Running scp upload with args: %v", args)
	cmd := exec.Command("scp", args...)
	return cmd.Run()
}

func ScpUploadFileWithSudo(cfg config.VMConfig, vmIP, src, dest string) error {
	// Create a temporary file on the VM to upload the file
	tmpFile, err := SshCommand(cfg, vmIP, "mktemp")
	if err != nil {
		return fmt.Errorf("failed to create temporary file on VM: %w", err)
	}
	// Use scp to upload file to temporary location
	if err = ScpUploadFile(cfg, vmIP, src, tmpFile); err != nil {
		return fmt.Errorf("failed to upload file to VM: %w", err)
	}
	// Move file to destination with sudo
	if _, err = SshCommand(cfg, vmIP, fmt.Sprintf("sudo mv %s %s", tmpFile, dest)); err != nil {
		return fmt.Errorf("failed to move file on VM: %w", err)
	}
	return nil
}

func innerSshCommand(cfg config.VMConfig, vmIP, command string, combineOutput bool, connectionRetryCount int, commandRetryCount int) (string, error) {
	sshCliSettings := sshconfig.SshCliSettings{
		PrivateKeyPath: cfg.SshPrivateKeyPath,
		Host:           vmIP,
		User:           cfg.User,
		Port:           22,
		Timeout:        5,
	}
	var err error
	client, err := retry.Retry(
		time.Second*time.Duration(connectionRetryCount),
		time.Second*time.Duration(1),
		func(attempt int) (*ssh.Client, error) {
			logrus.Tracef("SSH dial to '%s' (attempt %d)", sshCliSettings.FullHost(), attempt)
			return sshclient.OpenSshClient(sshCliSettings)
		},
	)
	if err != nil {
		return "", fmt.Errorf("failed to create SSH client: %w", err)
	}
	defer client.Close()

	output, err := retry.Retry(
		time.Second*time.Duration(commandRetryCount),
		time.Second*time.Duration(1),
		func(attempt int) (*string, error) {
			session, err := client.NewSession()
			if err != nil {
				return nil, fmt.Errorf("failed to create SSH session: %w", err)
			}
			defer session.Close()

			var output []byte
			if combineOutput {
				output, err = session.CombinedOutput(command)
			} else {
				output, err = session.Output(command)
			}
			sanitizedOutput := strings.TrimSpace(string(output))

			if err != nil {
				return &sanitizedOutput, fmt.Errorf("failed to run command '%s': %w\nOutput: %s", command, err, output)
			}
			return &sanitizedOutput, nil
		},
	)
	if err != nil {
		return "", fmt.Errorf("failed to run command '%s' on VM '%s': %w", command, vmIP, err)
	}
	if output == nil {
		return "", fmt.Errorf("no output received from command '%s' on VM '%s'", command, vmIP)
	}
	logrus.Tracef("SSH command '%s' output on VM '%s': %s", command, vmIP, *output)
	return *output, nil
}
