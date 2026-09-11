package helpers

import (
	"time"

	"tridenttools/storm/utils/metrics"
	stormretry "tridenttools/storm/utils/retry"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type BootMetricsHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		trident.RuntimeCliSettings    `embed:""`
		MetricsFile                   string `required:"" help:"Metrics file." type:"path"`
		MetricsOperation              string `required:"" help:"Metrics operation."`
	}
}

func (h BootMetricsHelper) Name() string {
	return "boot-metrics"
}

func (h *BootMetricsHelper) Args() any {
	return &h.args
}

func (h *BootMetricsHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("collect-boot-metrics", h.collectBootMetrics)
	return nil
}

func (h *BootMetricsHelper) collectBootMetrics(tc storm.TestCase) error {
	if h.args.TridentRuntimeType == trident.RuntimeTypeNone {
		tc.Skip("No Trident environment specified")
	}
	logrus.Infof("Waiting for the host to reboot and come back online...")

	// Each attempt dials a fresh client, so the retry tolerates the host still
	// rebooting when the helper starts.
	value, err := stormretry.Retry(
		time.Second*time.Duration(h.args.Timeout),
		time.Second*5,
		func(attempt int) (*metrics.BootMetric, error) {
			result := metrics.BootMetric{}
			client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
			if err != nil {
				return &result, err
			}

			output, err := stormsshclient.RunCommand(client, metrics.SystemdAnalyzeCommand)
			if err != nil {
				return &result, err
			}

			result, err = metrics.ParseBootMetric(h.args.MetricsOperation, output.Stdout)
			return &result, err
		},
	)
	if err != nil {
		tc.FailFromError(err)
	}

	if err := metrics.AppendBootMetrics(h.args.MetricsFile, *value); err != nil {
		tc.FailFromError(err)
	}

	return nil
}
