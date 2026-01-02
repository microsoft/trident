package helpers

import (
	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"

	stormsshcheck "tridenttools/storm/utils/ssh/check"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/trident"
)

type CheckSshHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		trident.RuntimeCliSettings    `embed:""`
		ExpectFailedCommit            bool `help:"Controls whether this test treats failed commits as successful." default:"false"`
	}
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
	err := stormsshcheck.CheckTridentService(
		h.args.SshCliSettings,
		h.args.TridentRuntimeType,
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
