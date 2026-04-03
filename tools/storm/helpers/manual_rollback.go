package helpers

import (
	"fmt"
	"os"
	"strings"

	stormsshcheck "tridenttools/storm/utils/ssh/check"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormsftp "tridenttools/storm/utils/ssh/sftp"
	"tridenttools/storm/utils/trident"
	stormtrident "tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"
)

type ManualRollbackHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		trident.RuntimeCliSettings    `embed:""`
		EnvVars                       []string `short:"e" help:"Environment variables. Multiple vars can be passed as a list of comma-separated strings, or this flag can be used multiple times. Each var should include the env var name, i.e. HTTPS_PROXY=http://0.0.0.0."`
		VmName                        string   `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		ExpectRuntimeRollback         bool     `help:"Whether to expect a runtime rollback to occur during the test." default:"false"`
	}
}

func (h ManualRollbackHelper) Name() string {
	return "manual-rollback"
}

func (h *ManualRollbackHelper) Args() any {
	return &h.args
}

func (h *ManualRollbackHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("rollback", h.rollback)
	return nil
}

func (h *ManualRollbackHelper) rollback(tc storm.TestCase) error {
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	// Get current configuration
	output, err := stormtrident.InvokeTrident(h.args.TridentRuntimeType, client, []string{}, "get configuration")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident 'get configuration' stderr:\n%s", output.Stderr)
		return err
	}
	logrus.Infof("Trident 'get configuration' output:\n%s", output.Stdout)

	// Check for available rollbacks
	output, err = stormtrident.InvokeTrident(h.args.TridentRuntimeType, client, []string{}, "get rollback-chain")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident 'get rollback-chain' stderr:\n%s", output.Stderr)
		return err
	}
	logrus.Infof("Trident 'get rollback-chain' output:\n%s", output.Stdout)
	var availableRollbacks []map[string]interface{}
	err = yaml.Unmarshal([]byte(strings.TrimSpace(output.Stdout)), &availableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to unmarshal available rollbacks: %w", err)
	}
	logrus.Infof("Available rollbacks: '%d'", len(availableRollbacks))

	tmpRollbackChainFile, err := os.CreateTemp("", "rollback-chain")
	if err != nil {
		return fmt.Errorf("failed to create local rollback chain file: %w", err)
	}
	defer tmpRollbackChainFile.Close()
	if err := os.WriteFile(tmpRollbackChainFile.Name(), []byte(output.Stdout), 0644); err != nil {
		return fmt.Errorf("failed to write local rollback chain file: %w", err)
	}
	tc.ArtifactBroker().PublishLogFile("rollback_chain.yaml", tmpRollbackChainFile.Name())

	// Get pre-rollback datastore
	if err := copyRemoteFileToArtifacts(client, "/var/lib/trident/datastore.sqlite", "pre-rollback-datastore.sqlite", tc); err != nil {
		logrus.Errorf("Failed to copy pre-rollback datastore: %v", err)
		return err
	}

	// Execute rollback
	out, err := stormtrident.InvokeTrident(h.args.TridentRuntimeType, client, h.args.EnvVars, "rollback -v trace --allowed-operations stage")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := out.Check(); err != nil {
		logrus.Errorf("Trident 'rollback --allowed-operations stage' stderr:\n%s", out.Stderr)
		return err
	}
	// Get trident-full.log contents after rollback staging
	if err := copyRemoteFileToArtifacts(client, "/var/log/trident-full.log", "rollback-staging.log", tc); err != nil {
		logrus.Errorf("Failed to copy rollback staging log: %v", err)
		return err
	}

	out, err = stormtrident.InvokeTrident(h.args.TridentRuntimeType, client, h.args.EnvVars, "rollback -v trace --allowed-operations finalize")
	if err != nil {
		if err, ok := err.(*ssh.ExitMissingError); ok && strings.Contains(out.Stderr, trident.REBOOTING_LOG_MESSAGE) {
			// The connection closed without an exit code, and the output contains REBOOTING_LOG_MESSAGE.
			// This indicates that the host has rebooted.
			logrus.Infof("Host rebooted successfully")
		} else {
			// Some unknown error occurred.
			logrus.Errorf("Failed to invoke Trident: %s; %s", err, out.Report())
			return fmt.Errorf("failed to invoke Trident: %w", err)
		}
	}
	logrus.Infof("Trident 'rollback' succeeded:\n%s", out.Stdout)

	if !h.args.ExpectRuntimeRollback {
		err := stormsshcheck.CheckTridentService(
			h.args.SshCliSettings,
			h.args.TridentRuntimeType,
			true,
			h.args.TimeoutDuration(),
			tc,
		)
		if err != nil {
			logrus.Errorf("Trident service check via SSH failed: %s", err)
			tc.FailFromError(err)
			return err
		}
	}

	// Recreate ssh client after reboot
	client.Close()
	client, err = stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	// Verify rollback success
	output, err = stormtrident.InvokeTrident(h.args.TridentRuntimeType, client, []string{}, "get status")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident 'get status' stderr:\n%s", output.Stderr)
		return err
	}

	// Get trident-full.log contents after rollback reboot
	if err := copyRemoteFileToArtifacts(client, "/var/log/trident-full.log", "rollback-commit.log", tc); err != nil {
		logrus.Errorf("Failed to copy rollback commit log: %v", err)
		return err
	}

	return nil
}

func copyRemoteFileToArtifacts(client *ssh.Client, remotePath string, artifactName string, tc storm.TestCase) error {
	localPath, err := stormsftp.DownloadRemoteFile(client, remotePath, "")
	if err != nil {
		return fmt.Errorf("failed to download remote file (%s): %w", remotePath, err)
	}
	tc.ArtifactBroker().PublishLogFile(artifactName, localPath)
	return nil
}
