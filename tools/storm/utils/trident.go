package utils

import (
	"fmt"
	"strings"
	"time"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

const (
	TRIDENT_BINARY    = "/usr/bin/trident"
	TRIDENT_CONTAINER = "docker run --pull=never --rm --privileged " +
		"-v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident " +
		"-v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys -v /var/log:/var/log " +
		"-v /etc/pki:/etc/pki:ro --pid host --ipc host trident/trident:latest"
	DOCKER_IMAGE_PATH = "/var/lib/trident/trident-container.tar.gz"
)

// Invokes Trident in the specified environment using the provided SSH session with the given arguments.
// It returns the output of the command execution, including stdout, stderr, and exit status.
//
// This function will NOT return an error if the command execution fails with a non-zero exit status.
//
// It only returns an error when:
// - The environment is invalid
// - The SSH session cannot be created
// - There was an error starting the command.
// - Some IO error occurred while reading stdout or stderr.
func InvokeTrident(env TridentEnvironment, client *ssh.Client, proxy string, arguments string) (*SshCmdOutput, error) {
	var cmd string
	switch env {
	case TridentEnvironmentHost:
		cmd = TRIDENT_BINARY
	case TridentEnvironmentContainer:
		cmd = TRIDENT_CONTAINER
	case TridentEnvironmentNone:
		return nil, fmt.Errorf("trident service is not running")
	default:
		return nil, fmt.Errorf("invalid environment: %s", env)
	}

	var cmdPrefix string
	if proxy != "" {
		envVar := strings.Split(proxy, "=")[0]
		cmdPrefix = fmt.Sprintf("%s sudo --preserve-env=%s", proxy, envVar)
	} else {
		cmdPrefix = "sudo"
	}

	return RunCommand(client, fmt.Sprintf("%s %s %s", cmdPrefix, cmd, arguments)) // possible to prepend env vars before "sudo"?
}

// Loads the Trident container stored in DOCKER_IMAGE_PATH int the remote host's
// Docker daemon by invoking the `docker load` command. It returns an error if
// the command fails or if the SSH client is nil. The function checks if the
// image is already loaded by running `docker images` to avoid reloading it
// unnecessarily.
func LoadTridentContainer(client *ssh.Client) error {
	if client == nil {
		return fmt.Errorf("SSH client is nil")
	}

	out, err := RunCommand(client, fmt.Sprintf("sudo docker images --format json %s", DOCKER_IMAGE_PATH))
	if err != nil {
		return fmt.Errorf("failed to run docker images command: %w", err)
	}

	err = out.Check()
	if err != nil {
		return fmt.Errorf("failed to check docker images command: %w", err)
	}

	if strings.TrimSpace(out.Stdout) != "" {
		// Image is already loaded, no need to load it again.
		return nil
	}

	// Load the image
	out, err = RunCommand(client, fmt.Sprintf("sudo docker load --input %s", DOCKER_IMAGE_PATH))
	if err != nil {
		return fmt.Errorf("failed to load docker image: %w", err)
	}
	err = out.Check()
	if err != nil {
		return fmt.Errorf("failed to check docker load command: %w", err)
	}

	return nil
}

func CheckTridentService(client *ssh.Client, env TridentEnvironment, timeout time.Duration) error {
	if client == nil {
		return fmt.Errorf("SSH client is nil")
	}

	var serviceName string
	switch env {
	case TridentEnvironmentHost:
		serviceName = "trident.service"
	case TridentEnvironmentContainer:
		serviceName = "trident-container.service"
	default:
		return fmt.Errorf("unsupported environment: %s", env)
	}

	_, err := Retry(
		timeout,
		time.Second*5,
		func(attempt int) (*bool, error) {
			logrus.Infof("Checking Trident service status (attempt %d)", attempt)
			err := checkTridentServiceInner(client, serviceName)
			if err != nil {
				logrus.Warnf("Trident service is not in expected state: %s", err)
				return nil, err
			}

			return nil, nil
		},
	)
	if err != nil {
		return fmt.Errorf("trident service is not in expected state: %w", err)
	}

	return nil
}

func checkTridentServiceInner(client *ssh.Client, serviceName string) error {
	session, err := client.NewSession()
	if err != nil {
		return fmt.Errorf("failed to create SSH session: %w", err)
	}
	defer session.Close()

	cmd := fmt.Sprintf("sudo systemctl status %s --no-pager", serviceName)
	logrus.Debugf("Running command: %s", cmd)

	output, err := session.CombinedOutput(cmd)
	if err != nil {
		// We expect systemctl to return an exit code of 3 when the service is
		// not running. This is expected after trident is finished. It is NOT an
		// error!
		if exitErr, ok := err.(*ssh.ExitError); !(ok && exitErr.ExitStatus() == 3) {
			// This is an unknown error, return it.
			logrus.Debugf("Received output:\n %s", output)
			return fmt.Errorf("failed to check Trident service status: %w", err)
		}
	}

	outputStr := string(output)

	logrus.Debugf("Trident service status:\n%s", outputStr)

	if !strings.Contains(outputStr, "Active: inactive (dead)") {
		return fmt.Errorf("expected to find 'Active: inactive (dead)' in Trident service status")
	}

	mainPidLine := ""
	lines := strings.Split(outputStr, "\n")
	for _, line := range lines {
		if strings.Contains(line, "Main PID:") {
			mainPidLine = line
			break
		}
	}

	if mainPidLine == "" {
		return fmt.Errorf("expected to find 'Main PID:' in Trident service status")
	}

	if !strings.Contains(mainPidLine, "(code=exited, status=0/SUCCESS") {
		return fmt.Errorf("expected to find '(code=exited, status=0/SUCCESS)' in Trident service status")
	}

	logrus.Info("Trident service ran successfully")

	return nil
}
