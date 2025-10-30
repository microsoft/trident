package helpers

import (
	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/env"
	check "tridenttools/storm/utils/ssh/check"
	sshconfig "tridenttools/storm/utils/ssh/config"
)

type CheckSshHelper struct {
	args struct {
		sshconfig.SshCliSettings `embed:""`
		env.EnvCliSettings       `embed:""`
		CheckActiveVolume        string `help:"Check that the indicated volume is the active one"`
		ExpectFailedCommit       bool   `help:"Controls whether this test treats failed commits as successful." default:"false"`
	}
	client *ssh.Client
}

func (h CheckSshHelper) Name() string {
	return "check-ssh"
}

func (h *CheckSshHelper) Args() any {
	return &h.args
}

func (h *CheckSshHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("check-ssh", h.checkTridentServiceWithSsh)
	return nil
}

func (h *CheckSshHelper) checkTridentServiceWithSsh(tc storm.TestCase) error {
	expectSuccessfulCommit := !h.args.ExpectFailedCommit
	err := check.CheckTridentService(
		h.args.SshCliSettings,
		h.args.EnvCliSettings,
		expectSuccessfulCommit,
		h.args.TimeoutDuration(),
		tc,
	)
	if err != nil {
		logrus.Errorf("Trident service check via SSH failed: %s", err)
		tc.FailFromError(err)
	}
	return nil
}
