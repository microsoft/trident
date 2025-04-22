package helpers

import (
	"fmt"
	"path"
	"regexp"
	"storm/pkg/storm"
	"storm/suites/trident/utils"
	"strings"
	"time"

	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"
)

type AbUpdateHelper struct {
	args struct {
		utils.SshCliSettings `embed:""`
		utils.EnvCliSettings `embed:""`
		DestinationDirectory string `short:"d" required:"" help:"Read-write directory on the host that contains the runtime OS images for the A/B update."`
		TridentConfig        string `short:"c" required:"" help:"File name of the custom read-write Trident config on the host to point Trident to."`
		Version              string `short:"v" required:"" help:"Version of the Trident image to use for the A/B update."`
		StageAbUpdate        bool   `short:"s" help:"Controls whether A/B update should be staged."`
		FinalizeAbUpdate     bool   `short:"f" help:"Controls whether A/B update should be finalized."`
	}

	client *ssh.Client
	config map[string]interface{}
}

func (h AbUpdateHelper) Name() string {
	return "ab-update"
}

func (h *AbUpdateHelper) Args() any {
	return &h.args
}

func (h *AbUpdateHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("get-config", h.getHostConfig)
	r.RegisterTestCase("update-hc", h.updateHostConfig)
	r.RegisterTestCase("trigger-update", h.triggerTridentUpdate)
	r.RegisterTestCase("check-trident-service", h.checkTridentService)
	return nil
}

func (h *AbUpdateHelper) getHostConfig(tc storm.TestCase) error {
	if h.args.Env == utils.TridentEnvironmentNone {
		return fmt.Errorf("environment %s is not supported", h.args.Env)
	}

	var err error
	h.client, err = utils.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
	}

	tc.SuiteCleanup(func() {
		if h.client != nil {
			h.client.Close()
		}
	})

	out, err := utils.InvokeTrident(h.args.Env, h.client, "get configuration")
	if err != nil {
		return fmt.Errorf("failed to invoke Trident: %w", err)
	}

	if err := out.Check(); err != nil {
		return fmt.Errorf("failed to run trident to get host config: %w", err)
	}

	tc.Logger().Debugf("Trident stdout:\n%s", out.Stdout)
	tc.Logger().Debugf("Trident stderr:\n%s", out.Stderr)

	err = yaml.Unmarshal([]byte(out.Stdout), &h.config)
	if err != nil {
		return fmt.Errorf("failed to unmarshal YAML: %w", err)
	}
	tc.Logger().Infof("Trident configuration: %v", h.config)

	return nil
}

func (h *AbUpdateHelper) updateHostConfig(tc storm.TestCase) error {
	if !h.args.StageAbUpdate {
		tc.Skip("Staging not requested")
	}

	// Extract the OLD URL from the configuration
	oldUrl, ok := h.config["image"].(map[string]interface{})["url"].(string)
	if !ok {
		return fmt.Errorf("failed to get old image URL from configuration")
	}

	tc.Logger().Infof("Old image URL: %s", oldUrl)

	base := path.Base(oldUrl)

	matches := regexp.MustCompile(`^(.*?)(_v\d+)?\.(.+)$`).FindStringSubmatch(base)

	if len(matches) != 4 {
		return fmt.Errorf("failed to parse image name: %s", base)
	}

	name := matches[1]
	ext := matches[3]

	newCosiName := fmt.Sprintf("%s_v%s.%s", name, h.args.Version, ext)
	newCosiPath := path.Join(h.args.DestinationDirectory, newCosiName)
	tridentCosiPath := path.Join(h.args.Env.HostPath(), newCosiPath)
	newUrl := fmt.Sprintf("file://%s", tridentCosiPath)
	tc.Logger().Infof("New image URL: %s", newUrl)

	// Update the image URL in the configuration
	h.config["image"].(map[string]any)["url"] = newUrl

	// Set the config to NOT self-upgrade
	trident, ok := h.config["trident"].(map[string]any)
	if !ok {
		trident = make(map[string]any)
		h.config["trident"] = trident
	}

	trident["selfUpgrade"] = false

	// Delete the storage section from the config, not needed for A/B update
	delete(h.config, "storage")

	hc_yaml, err := yaml.Marshal(h.config)
	if err != nil {
		return fmt.Errorf("failed to marshal YAML: %w", err)
	}

	sftpClient, err := utils.NewSftpSudoClient(h.client)
	if err != nil {
		return fmt.Errorf("failed to create SudoSFTP client: %w", err)
	}
	defer sftpClient.Close()

	// Ensure the cosi file exists
	tc.Logger().Infof("Checking if new COSI file exists at %s", newCosiPath)
	_, err = sftpClient.Stat(newCosiPath)
	if err != nil {
		fmt.Println("Yielding to the error")
		return fmt.Errorf("failed to stat new COSI file at %s: %w", newCosiPath, err)
	}

	err = sftpClient.MkdirAll(path.Dir(h.args.TridentConfig))
	if err != nil {
		return fmt.Errorf("failed to create directory for new Host Config file: %w", err)
	}

	file, err := sftpClient.Create(h.args.TridentConfig)
	if err != nil {
		return fmt.Errorf("failed to create new Host Config file: %w", err)
	}
	defer file.Close()

	_, err = file.Write(hc_yaml)
	if err != nil {
		return fmt.Errorf("failed to write new Host Config file: %w", err)
	}

	err = file.Chmod(0644)
	if err != nil {
		return fmt.Errorf("failed to change permissions of new Host Config file: %w", err)
	}

	err = file.Chown(0, 0)
	if err != nil {
		return fmt.Errorf("failed to change ownership of new Host Config file: %w", err)
	}

	return nil
}

func (h *AbUpdateHelper) triggerTridentUpdate(tc storm.TestCase) error {
	allowedOperations := make([]string, 0)

	if h.args.StageAbUpdate {
		tc.Logger().Infof("Allowed operations: stage")
		allowedOperations = append(allowedOperations, "stage")
	}

	if h.args.FinalizeAbUpdate {
		tc.Logger().Infof("Allowed operations: finalize")
		allowedOperations = append(allowedOperations, "finalize")
	}

	args := fmt.Sprintf(
		"update -v trace %s --allowed-operations %s",
		path.Join(h.args.Env.HostPath(), h.args.TridentConfig),
		strings.Join(allowedOperations, ","),
	)

	file, err := utils.CommandOutput(h.client, tc.Logger(), fmt.Sprintf("sudo cat %s", h.args.TridentConfig))
	if err != nil {
		return fmt.Errorf("failed to read new Host Config file: %w", err)
	}

	tc.Logger().Debugf("Trident config file:\n%s", file)

	for i := 1; ; i++ {
		tc.Logger().Infof("Invoking Trident attempt #%d with args: %s", i, args)

		out, err := utils.InvokeTrident(h.args.Env, h.client, args)
		if err != nil {
			if err, ok := err.(*ssh.ExitMissingError); ok && strings.Contains(out.Stderr, "Rebooting system") {
				// The connection closed without an exit code, and the output contains "Rebooting system".
				// This indicates that the host has rebooted.
				tc.Logger().Infof("Host rebooted successfully")
				break
			} else {
				// Some unknown error occurred.
				tc.Logger().Errorf("Failed to invoke Trident: %s; %s", err, out.Report())
				return fmt.Errorf("failed to invoke Trident: %w", err)
			}
		}

		if out.Status == 0 && strings.Contains(out.Stderr, "Staging of update 'AbUpdate' succeeded") {
			tc.Logger().Infof("Staging of update 'AbUpdate' succeeded")
			break
		}

		if out.Status == 2 && strings.Contains(out.Stderr, "Failed to run post-configure script 'fail-on-the-first-run'") {
			tc.Logger().Infof("Detected intentional failure. Re-running...")
			continue
		}

		tc.Logger().Errorf("Trident update failed %s", out.Report())

		tc.Fail(fmt.Sprintf("Trident update failed with status %d", out.Status))
	}

	// On success close the client because the host will reboot into the new OS.
	h.client.Close()
	h.client = nil

	return nil
}

func (h *AbUpdateHelper) checkTridentService(tc storm.TestCase) error {
	if h.args.Env == utils.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}

	tc.Logger().Infof("Waiting for the host to reboot and come back online...")
	time.Sleep(time.Second * 10)

	// Reconnect via SSH to the updated OS
	_, err := utils.Retry(
		h.args.TimeoutDuration(),
		time.Second*5,
		func(attempt int) (*bool, error) {
			tc.Logger().Infof("SSH dial to '%s' (attempt %d)", h.args.SshCliSettings.FullHost(), attempt)
			client, err := utils.OpenSshClient(h.args.SshCliSettings)
			if err != nil {
				tc.Logger().Warnf("Failed to dial SSH server '%s': %s", h.args.SshCliSettings.FullHost(), err)
				return nil, err
			}
			defer client.Close()

			tc.Logger().Infof("SSH dial to '%s' succeeded", h.args.SshCliSettings.FullHost())

			err = utils.CheckTridentService(client, tc.Logger(), h.args.Env, h.args.TimeoutDuration())
			if err != nil {
				tc.Logger().Warnf("Trident service is not in expected state: %s", err)
				return nil, err
			}

			tc.Logger().Infof("Trident service is in expected state")
			return nil, nil
		},
	)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	return nil
}
