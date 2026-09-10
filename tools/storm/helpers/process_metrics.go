package helpers

import (
	"fmt"
	"os"

	"tridenttools/storm/utils/metrics"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

// ProcessMetricsHelper ports tests/e2e_tests/helpers/process_trident_metrics.py
// together with the "metrics file must not be empty" assertion the legacy
// trident-metrics.yml step performed around it. It runs as a helper rather than
// as a case inside the E2E scenario for two reasons: the pipeline invokes it
// with condition succeededOrFailed(), so metrics still reach Kusto for a failed
// config (storm skips a scenario's remaining cases once one fails); and a local
// dev-box run never schedules it at all, because only the pipeline invokes it.
type ProcessMetricsHelper struct {
	args struct {
		HostConfig   string   `required:"" name:"host-config" help:"Path to the Host Configuration the metrics were produced for." type:"path"`
		MetricsFiles []string `required:"" name:"metrics-file" help:"Path to a trace-stream metrics file to enrich. May be repeated." type:"path"`

		// Legacy failed the task when the metrics file was empty, but only for
		// operations that were expected to produce metrics at all.
		RequireNonEmpty bool `name:"require-non-empty" help:"Fail if a metrics file is missing or empty."`

		// Pipeline context. Bound to the same environment variables the legacy
		// script read, so the pipeline needs no extra wiring, while a local
		// invocation can still override any of them explicitly.
		PipelineName      string `name:"pipeline-name" env:"PIPELINE_NAME" default:"Unknown"`
		PipelineBuildId   string `name:"pipeline-build-id" env:"BUILD_BUILDID" default:"Unknown"`
		PipelineAgentSku  string `name:"pipeline-agent-sku" env:"PIPELINE_AGENT_SKU" default:"Unknown"`
		Environment       string `name:"environment" env:"TEST_ENVIRONMENT" default:"Unknown"`
		Location          string `name:"location" env:"TEST_LOCATION" default:"Unknown"`
		ServerName        string `name:"server-name" env:"TEST_SERVER_NAME" default:"Unknown"`
		Branch            string `name:"branch" env:"SOURCE_BRANCH_NAME" default:"Unknown"`
		MachineType       string `name:"machine-type" env:"MACHINE_TYPE" default:"Unknown"`
		RuntimeEnv        string `name:"runtime-env" env:"RUNTIME_ENVIRONMENT" default:"Unknown"`
		TridentConfigName string `name:"trident-config-name" env:"TRIDENT_CONFIGURATION_NAME" default:"Unknown"`
		TridentCommitHash string `name:"trident-commit-hash" env:"BUILD_SOURCEVERSION" default:"Unknown"`

		// Tags every record so dashboards can tell storm-produced rows apart
		// from the legacy suite's while both run against the same table.
		TestRunner string `name:"test-runner" help:"Value recorded as additional_fields.test_runner." default:"storm"`
	}
}

func (h ProcessMetricsHelper) Name() string {
	return "process-metrics"
}

func (h *ProcessMetricsHelper) Args() any {
	return &h.args
}

func (h *ProcessMetricsHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("process-metrics", h.processMetrics)
	return nil
}

func (h *ProcessMetricsHelper) processMetrics(tc storm.TestCase) error {
	config, err := metrics.LoadHostConfig(h.args.HostConfig)
	if err != nil {
		tc.FailFromError(err)
	}

	enrichment := metrics.NewEnrichment(metrics.PipelineContext{
		PipelineName:      h.args.PipelineName,
		PipelineBuildId:   h.args.PipelineBuildId,
		PipelineAgentSku:  h.args.PipelineAgentSku,
		Environment:       h.args.Environment,
		Location:          h.args.Location,
		ServerName:        h.args.ServerName,
		Branch:            h.args.Branch,
		MachineType:       h.args.MachineType,
		RuntimeEnv:        h.args.RuntimeEnv,
		TridentConfigName: h.args.TridentConfigName,
		TridentCommitHash: h.args.TridentCommitHash,
		TestRunner:        h.args.TestRunner,
	}, &config)

	var missing []string
	for _, path := range h.args.MetricsFiles {
		empty, err := isEmptyOrMissing(path)
		if err != nil {
			tc.FailFromError(err)
		}
		if empty {
			// Legacy skipped enrichment for an empty file and only failed when
			// the operation was expected to produce metrics.
			logrus.Warnf("Metrics file '%s' is missing or empty, nothing to enrich", path)
			missing = append(missing, path)
			continue
		}

		count, err := metrics.EnrichFile(path, enrichment)
		if err != nil {
			tc.FailFromError(err)
		}
		logrus.Infof("Enriched %d metric record(s) in '%s'", count, path)
	}

	if h.args.RequireNonEmpty && len(missing) > 0 {
		tc.Fail(fmt.Sprintf("expected metrics but these files are missing or empty: %v", missing))
	}

	return nil
}

// isEmptyOrMissing reports whether a metrics file is absent or has no content,
// which is how a servicing operation that never reported shows up on disk.
func isEmptyOrMissing(path string) (bool, error) {
	info, err := os.Stat(path)
	if os.IsNotExist(err) {
		return true, nil
	}
	if err != nil {
		return false, fmt.Errorf("failed to stat metrics file '%s': %w", path, err)
	}
	if info.IsDir() {
		return false, fmt.Errorf("metrics file '%s' is a directory", path)
	}
	return info.Size() == 0, nil
}
