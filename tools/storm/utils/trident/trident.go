package trident

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	stormssh "github.com/microsoft/storm/utils/ssh"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	stormretry "tridenttools/storm/utils/retry"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/sshutils"
)

const (
	TRIDENT_BINARY      = "/usr/bin/trident"
	DOCKER_COMMAND_BASE = "docker run --pull=never --rm --privileged " +
		"-v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident " +
		"-v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys -v /var/log:/var/log " +
		"-v /etc/pki:/etc/pki:ro --pid host --ipc host "
	TRIDENT_CONTAINER = "trident/trident:latest"
	DOCKER_IMAGE_PATH = "/var/lib/trident/trident-container.tar.gz"

	// MESSAGE logged by Trident before requesting a system reboot.
	// Must match the message in crates/trident/src/reboot.rs
	REBOOTING_LOG_MESSAGE = "Requesting reboot"
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
func InvokeTrident(runtime RuntimeType, client sshutils.CommandRunner, envVars []string, arguments string) (*stormsshconfig.SshCmdOutput, error) {
	var cmd string
	switch runtime {
	case RuntimeTypeHost:
		cmd = TRIDENT_BINARY
	case RuntimeTypeContainer:
		cmd = BuildTridentContainerCommand(envVars)
	case RuntimeTypeNone:
		return nil, fmt.Errorf("trident service is not running")
	default:
		return nil, fmt.Errorf("invalid environment: %s", runtime)
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
	return stormsshclient.RunCommand(client, fmt.Sprintf("%s %s %s", cmdPrefix, cmd, arguments))
}

// CheckCommandResult accepts a reboot disconnect only when Trident announced that reboot.
func CheckCommandResult(out *stormsshconfig.SshCmdOutput, err error, expectReboot bool) error {
	var disconnected *ssh.ExitMissingError
	if expectReboot && out != nil && errors.As(err, &disconnected) &&
		strings.Contains(out.Stderr, REBOOTING_LOG_MESSAGE) {
		return nil
	}
	if err != nil {
		return err
	}
	if out == nil {
		return fmt.Errorf("Trident returned no command result")
	}
	return out.Check()
}

// Loads the Trident container stored in DOCKER_IMAGE_PATH int the remote host's
// Docker daemon by invoking the `docker load` command. It returns an error if
// the command fails or if the SSH client is nil. The function checks if the
// image is already loaded by running `docker images` to avoid reloading it
// unnecessarily.
func LoadTridentContainer(client *stormssh.Client) error {
	if client == nil {
		return fmt.Errorf("SSH client is nil")
	}

	// Filter by image reference, not by the tarball path: `docker images`
	// treats its positional argument as a repository reference, so passing a
	// file path matches nothing and the load below would run on every
	// reconnect. That matters because this is called from populateSshClient,
	// which re-dials after every reboot and on every retry that drops its
	// client.
	out, err := stormsshclient.RunCommand(client, fmt.Sprintf("sudo docker images --format json %s", TRIDENT_CONTAINER))
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
	out, err = stormsshclient.RunCommand(client, fmt.Sprintf("sudo docker load --input %s", DOCKER_IMAGE_PATH))
	if err != nil {
		return fmt.Errorf("failed to load docker image: %w", err)
	}
	err = out.Check()
	if err != nil {
		return fmt.Errorf("failed to check docker load command: %w", err)
	}

	return nil
}

func CheckTridentService(client *stormssh.Client, runtime RuntimeType, timeout time.Duration, expectSuccessfulCommit bool) error {
	if client == nil {
		return fmt.Errorf("SSH client is nil")
	}

	var serviceName string
	switch runtime {
	case RuntimeTypeHost:
		serviceName = "trident.service"
	case RuntimeTypeContainer:
		serviceName = "trident-container.service"
	default:
		return fmt.Errorf("unsupported environment: %s", runtime)
	}

	reconnectNeeded, err := stormretry.Retry(
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
func checkTridentServiceInner(client *stormssh.Client, serviceName string, expectSuccessfulCommit bool) (bool, error) {
	cmd := fmt.Sprintf("sudo systemctl status %s --no-pager", serviceName)
	logrus.Debugf("Running command: %s", cmd)

	result, err := client.Run(context.Background(), cmd)
	if err != nil {
		return true, fmt.Errorf("failed to check Trident service status: %w", err)
	}
	// systemctl returns 3 for the expected inactive/failed commit service.
	if result.ExitCode != 0 && result.ExitCode != 3 {
		return false, fmt.Errorf("systemctl status exited %d: %s", result.ExitCode, result.Stderr)
	}

	outputStr := string(result.Stdout) + string(result.Stderr)

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
