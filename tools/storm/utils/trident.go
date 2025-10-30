package utils

import (
	"fmt"
	"strings"
	"time"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

const (
	TRIDENT_BINARY      = "/usr/bin/trident"
	DOCKER_COMMAND_BASE = "docker run --pull=never --rm --privileged " +
		"-v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident " +
		"-v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys -v /var/log:/var/log " +
		"-v /etc/pki:/etc/pki:ro --pid host --ipc host "
	TRIDENT_CONTAINER = "trident/trident:latest"
	DOCKER_IMAGE_PATH = "/var/lib/trident/trident-container.tar.gz"
)

func BuildTridentContainerCommand(envVars []string) string {
	cmd := DOCKER_COMMAND_BASE
	if len(envVars) != 0 {
		for _, envVar := range envVars {
			cmd += fmt.Sprintf("--env '%s' ", envVar)
		}
	}
	cmd += TRIDENT_CONTAINER
	return cmd
}

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
func InvokeTrident(env TridentEnvironment, client *ssh.Client, envVars []string, arguments string) (*SshCmdOutput, error) {
	var cmd string
	switch env {
	case TridentEnvironmentHost:
		cmd = TRIDENT_BINARY
	case TridentEnvironmentContainer:
		cmd = BuildTridentContainerCommand(envVars)
	case TridentEnvironmentNone:
		return nil, fmt.Errorf("trident service is not running")
	default:
		return nil, fmt.Errorf("invalid environment: %s", env)
	}

	var cmdPrefix string
	if len(envVars) != 0 {
		var quotedEnvVars = ""
		for _, v := range envVars {
			quotedEnvVars += fmt.Sprintf("'%s' ", v)
		}
		cmdPrefix = fmt.Sprintf("sudo %s", quotedEnvVars)
	} else {
		cmdPrefix = "sudo"
	}

	logrus.Debug(fmt.Sprintf("Running command: %s %s %s", cmdPrefix, cmd, arguments))
	return RunCommand(client, fmt.Sprintf("%s %s %s", cmdPrefix, cmd, arguments))
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

func CheckTridentService(client *ssh.Client, env TridentEnvironment, timeout time.Duration, expectSuccessfulCommit bool) error {
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

	reconnectNeeded, err := Retry(
		timeout,
		time.Second*5,
		func(attempt int) (*bool, error) {
			logrus.Infof("Checking Trident service status (attempt %d)", attempt)
			reconnect, err := checkTridentServiceInner(client, serviceName, expectSuccessfulCommit)
			if reconnect {
				return &reconnect, nil
			}
			if err != nil {
				logrus.Warnf("Trident service is not in expected state: %s", err)
				return nil, err
			}

			return nil, nil
		},
	)
	if reconnectNeeded != nil && *reconnectNeeded {
		return fmt.Errorf("SSH connection needs to be re-established")
	}
	if err != nil {
		return fmt.Errorf("trident service is not in expected state: %w", err)
	}

	return nil
}

// checkTridentServiceInner checks the status of the Trident service on the host.
//   - client: An established SSH client connection to the host. This is used to
//     create new SSH sessions to execute commands. If the client fails to
//     create a new session, 'true' is returned to indicate that the SSH client
//     needs to be recreated.
//   - serviceName: The name of the Trident service to check. This is expected to differ
//     based on whether Trident is running in a container or on the host.
//   - expectSuccessfulCommit: A boolean indicating what the expected result of
//     `trident commit` should be on the host,  successful (true) or a
//     failed (false).
//
// The function returns a boolean indicating whether the SSH connection needs to be
// re-established (typically when `client` fails to create a new SSH session) and an
// error if the service is not in the expected state or if there was an error
// executing the command.
func checkTridentServiceInner(client *ssh.Client, serviceName string, expectSuccessfulCommit bool) (bool, error) {
	session, err := client.NewSession()
	if err != nil {
		// New session was not created, return `true` to indicate that
		// the SSH client should be recreated and checkTridentServiceInner
		// should be retried.
		return true, fmt.Errorf("failed to create SSH session: %w", err)
	}
	defer session.Close()

	cmd := fmt.Sprintf("sudo systemctl status %s --no-pager", serviceName)
	logrus.Debugf("Running command: %s", cmd)

	output, err := session.CombinedOutput(cmd)
	if err != nil {
		logrus.Debugf("Received output:\n %s", output)
		// We expect systemctl to return an exit code of 3 when the service is
		// not running. This is expected after trident is finished. It is NOT an
		// error!
		if exitErr, ok := err.(*ssh.ExitError); !(ok && exitErr.ExitStatus() == 3) {
			tridentGetOutput, tridentGetErr := session.CombinedOutput("sudo trident get")
			logrus.Debugf("Host Status (err=%+v):\n%s", tridentGetErr, string(tridentGetOutput))

			// This is an unknown error, return it.
			logrus.Debugf("Received output:\n %s", output)
			return false, fmt.Errorf("failed to check Trident service status: %w", err)
		}
	}

	outputStr := string(output)

	logrus.Debugf("Trident service status:\n%s", outputStr)

	if expectSuccessfulCommit {
		if !strings.Contains(outputStr, "Active: inactive (dead)") {
			return false, fmt.Errorf("expected to find 'Active: inactive (dead)' in Trident service status")
		}
	} else {
		if !strings.Contains(outputStr, "Active: failed (Result: exit-code)") {
			return false, fmt.Errorf("expected to find 'Active: failed (Result: exit-code)' in Trident service status")
		}
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
		return false, fmt.Errorf("expected to find 'Main PID:' in Trident service status")
	}

	commitSuccessfulExit := strings.Contains(mainPidLine, "(code=exited, status=0/SUCCESS")
	if expectSuccessfulCommit {
		if !commitSuccessfulExit {
			// commit exited with non-zero status, but we expected success
			return false, fmt.Errorf("expected Trident service status to show '(code=exited, status=0/SUCCESS)', but it did not")
		} else {
			logrus.Info("Trident service ran and exited successfully")
		}
	} else {
		if commitSuccessfulExit {
			// we expected commit to exit with non-zero status, but we found success
			return false, fmt.Errorf("expected Trident service status to show non-zero exit status, but found '(code=exited, status=0/SUCCESS)'")
		} else {
			logrus.Info("Trident service ran as expected and exited with non-zero status")
		}
	}

	return false, nil
}
