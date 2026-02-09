package helpers

import (
	"fmt"
	"strings"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type CheckJournaldHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		trident.RuntimeCliSettings    `embed:""`
		SyslogIdentifier              string `help:"Syslog identifier to check for in journald logs." default:"trident-tracing"`
		MetricToCheck                 string `help:"Name of the metric to check for in journald logs." required:""`
	}
}

func (h CheckJournaldHelper) Name() string {
	return "check-journald"
}

func (h *CheckJournaldHelper) Args() any {
	return &h.args
}

func (h *CheckJournaldHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("check-journald", h.checkJournald)
	return nil
}

func (h *CheckJournaldHelper) checkJournald(tc storm.TestCase) error {
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	tridentJournaldTraceLogs, err := stormsshclient.RunCommand(client, "sudo journalctl -t "+h.args.SyslogIdentifier+" -o json-pretty")
	if err != nil {
		return err
	}

	collectedLogs := tridentJournaldTraceLogs.Stdout
	logrus.Infof("Journald logs for %s:\n%s\n", h.args.SyslogIdentifier, collectedLogs)

	// Check if the expected metric is present in the journald logs
	if !strings.Contains(collectedLogs, fmt.Sprintf("\"F_METRIC_NAME\" : \"%s\"", h.args.MetricToCheck)) {
		tc.Fail(fmt.Sprintf("Expected metric '%s' not found in journald logs for syslog identifier '%s'", h.args.MetricToCheck, h.args.SyslogIdentifier))
	}

	return nil
}
