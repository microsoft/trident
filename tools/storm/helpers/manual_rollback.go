package helpers

import (
	stormenv "tridenttools/storm/utils/env"
	stormsshcheck "tridenttools/storm/utils/ssh/check"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormtrident "tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type ManualRollbackHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		stormenv.EnvCliSettings       `embed:""`
		DeploymentEnvironment         string `help:"Deployment environment (e.g., bareMetal, virtualMachine)." type:"string" default:"virtualMachine"`
		VmName                        string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		ExpectRuntimeRollback         bool   `help:"Whether to expect a runtime rollback to occur during the test." default:"false"`
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

	// Check for available rollbacks
	output, err := stormtrident.InvokeTrident(h.args.Env, client, []string{}, "rollback --show chain")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident 'rollback --show chain' stderr:\n%s", output.Stderr)
		return err
	}

	// Execute rollback
	output, err = stormtrident.InvokeTrident(h.args.Env, client, []string{}, "rollback -v trace")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident 'rollback' stderr:\n%s", output.Stderr)
		return err
	}

	logrus.Info("Trident 'rollback' succeeded")
	logrus.Tracef("Trident 'rollback' output:\n%s\n%s", output.Stdout, output.Stderr)

	if !h.args.ExpectRuntimeRollback {
		err := stormsshcheck.CheckTridentService(
			h.args.SshCliSettings,
			h.args.EnvCliSettings,
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

	// Verify rollback success
	output, err = stormtrident.InvokeTrident(h.args.Env, client, []string{}, "get status")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident 'get status' stderr:\n%s", output.Stderr)
		return err
	}

	return nil
}
