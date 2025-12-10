package helpers

import (
	"fmt"
	"path"
	"strings"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"

	stormenv "tridenttools/storm/utils/env"
	stormsshcheck "tridenttools/storm/utils/ssh/check"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormsftp "tridenttools/storm/utils/ssh/sftp"
	stormtrident "tridenttools/storm/utils/trident"
)

type RuntimeUpdateHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		stormenv.EnvCliSettings       `embed:""`
		TridentConfig                 string   `short:"c" required:"" help:"File name of the custom read-write Trident config on the host to point Trident to."`
		StageRuntimeUpdate            bool     `short:"s" help:"Controls whether runtime update should be staged."`
		FinalizeRuntimeUpdate         bool     `short:"f" help:"Controls whether runtime update should be finalized."`
		EnvVars                       []string `short:"e" help:"Environment variables. Multiple vars can be passed as a list of comma-separated strings, or this flag can be used multiple times. Each var should include the env var name, i.e. HTTPS_PROXY=http://0.0.0.0."`
	}

	client *ssh.Client
	config map[string]interface{}
}

func (h RuntimeUpdateHelper) Name() string {
	return "runtime-update"
}

func (h *RuntimeUpdateHelper) Args() any {
	return &h.args
}

func (h *RuntimeUpdateHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("get-config", h.getHostConfig)
	r.RegisterTestCase("update-hc", h.updateHostConfig)
	r.RegisterTestCase("trigger-update", h.triggerTridentUpdate)
	r.RegisterTestCase("check-trident-service", h.checkTridentService)
	return nil
}

func (h *RuntimeUpdateHelper) getHostConfig(tc storm.TestCase) error {
	if h.args.Env == stormenv.TridentEnvironmentNone {
		return fmt.Errorf("environment %s is not supported", h.args.Env)
	}

	var err error
	h.client, err = stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
	}

	tc.SuiteCleanup(func() {
		if h.client != nil {
			h.client.Close()
		}
	})

	out, err := stormtrident.InvokeTrident(h.args.Env, h.client, h.args.EnvVars, "get configuration")
	if err != nil {
		return fmt.Errorf("failed to invoke Trident: %w", err)
	}

	if err := out.Check(); err != nil {
		return fmt.Errorf("failed to run trident to get host config: %w", err)
	}

	logrus.Debugf("Trident stdout:\n%s", out.Stdout)
	logrus.Debugf("Trident stderr:\n%s", out.Stderr)

	err = yaml.Unmarshal([]byte(out.Stdout), &h.config)
	if err != nil {
		return fmt.Errorf("failed to unmarshal YAML: %w", err)
	}
	logrus.Infof("Trident configuration: %v", h.config)

	return nil
}

func (h *RuntimeUpdateHelper) updateHostConfig(tc storm.TestCase) error {
	if !h.args.StageRuntimeUpdate {
		tc.Skip("Staging not requested")
	}

	// Set the config to NOT self-upgrade
	internalParams, ok := h.config["internalParams"].(map[string]any)
	if !ok {
		internalParams = make(map[string]any)
		h.config["internalParams"] = internalParams
	}
	internalParams["selfUpgradeTrident"] = false

	// Delete the storage section from the config, not needed for runtime update
	delete(h.config, "storage")
	// Remove any scripts
	delete(h.config, "scripts")

	// Retrieve the old netplan configuration
	osConfig, ok := h.config["os"].(map[string]interface{})
	if !ok {
		osConfig = make(map[string]any)
		h.config["os"] = osConfig
	}
	oldNetplan, ok := osConfig["netplan"].(map[string]interface{})
	if !ok {
		oldNetplan = make(map[string]any)
		osConfig["netplan"] = oldNetplan
	}

	// Add dhcp6 to the netplan configuration
	ethernets, ok := oldNetplan["ethernets"].(map[string]interface{})
	if !ok {
		ethernets = make(map[string]any)
		oldNetplan["ethernets"] = ethernets
	}
	vmeths, ok := ethernets["vmeths"].(map[string]interface{})
	if !ok {
		vmeths = make(map[string]any)
		ethernets["vmeths"] = vmeths
	}
	vmeths["dhcp6"] = true

	// Write the updated config
	hc_yaml, err := yaml.Marshal(h.config)
	if err != nil {
		return fmt.Errorf("failed to marshal YAML: %w", err)
	}

	sftpClient, err := stormsftp.NewSftpSudoClient(h.client)
	if err != nil {
		return fmt.Errorf("failed to create SudoSFTP client: %w", err)
	}
	defer sftpClient.Close()

	// Ensure the cosi file exists
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

func (h *RuntimeUpdateHelper) triggerTridentUpdate(tc storm.TestCase) error {
	allowedOperations := make([]string, 0)

	if h.args.StageRuntimeUpdate {
		logrus.Infof("Allowed operations: stage")
		allowedOperations = append(allowedOperations, "stage")
	}

	if h.args.FinalizeRuntimeUpdate {
		logrus.Infof("Allowed operations: finalize")
		allowedOperations = append(allowedOperations, "finalize")
	}

	args := fmt.Sprintf(
		"update -v trace %s --allowed-operations %s",
		path.Join(h.args.Env.HostPath(), h.args.TridentConfig),
		strings.Join(allowedOperations, ","),
	)

	file, err := stormsshclient.CommandOutput(h.client, fmt.Sprintf("sudo cat %s", h.args.TridentConfig))
	if err != nil {
		return fmt.Errorf("failed to read new Host Config file: %w", err)
	}

	logrus.Debugf("Trident config file:\n%s", file)

	for i := 1; ; i++ {
		logrus.Infof("Invoking Trident attempt #%d with args: %s", i, args)

		out, err := stormtrident.InvokeTrident(h.args.Env, h.client, h.args.EnvVars, args)
		if err != nil {
			logrus.Errorf("Failed to invoke Trident: %s; %s", err, out.Report())
			return fmt.Errorf("failed to invoke Trident: %w", err)
		}

		if out.Status == 0 && strings.Contains(out.Stderr, "Staging of runtime update succeeded") {
			logrus.Infof("Staging of runtime update succeeded")
			break
		}

		logrus.Errorf("Trident update failed %s", out.Report())

		tc.Fail(fmt.Sprintf("Trident update failed with status %d", out.Status))
	}

	// On success close the client because the host will reboot into the new OS.
	h.client.Close()
	h.client = nil

	return nil
}

func (h *RuntimeUpdateHelper) checkTridentService(tc storm.TestCase) error {
	if h.args.Env == stormenv.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}

	expectSuccessfulCommit := true
	err := stormsshcheck.CheckTridentService(
		h.args.SshCliSettings,
		h.args.EnvCliSettings,
		expectSuccessfulCommit,
		h.args.TimeoutDuration(),
		tc)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}
	return nil
}
