// Package process_metrics enriches the trace-stream metrics a storm E2E run
// produced so they carry the pipeline and Host Configuration context the Kusto
// `trident` table expects.
//
// It is a script rather than a helper because it performs an action rather than
// running test cases, and because a script's stdout is not captured by storm's
// test-case output redirection - which matters here, since it reports back to
// the pipeline through an Azure DevOps logging command (the same mechanism
// acr-push uses).
package process_metrics

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"tridenttools/storm/e2e/scenario"
	stormutils "tridenttools/storm/utils"
	"tridenttools/storm/utils/metrics"

	"github.com/sirupsen/logrus"
)

type ProcessMetricsScriptSet struct {
	ProcessMetrics ProcessMetricsScript `cmd:"" name:"process-metrics" help:"Enrich storm E2E trace-stream metrics for Kusto ingestion."`
}

type ProcessMetricsScript struct {
	// The run's artifact directory. prepare-hc publishes the deployed Host
	// Configuration here as its last action, so its absence means the run
	// died before deploying anything and there is nothing to enrich.
	ArtifactDir string `name:"artifact-dir" help:"Directory the storm run wrote its artifacts to." type:"path"`

	// Directory the scenario wrote its trace-stream files to. The files are
	// discovered rather than enumerated by the caller: which ones exist is a
	// property of the servicing phases the configuration ran.
	MetricsDir string `name:"metrics-dir" help:"Directory to discover trace-stream metrics files in." type:"path"`

	// Explicit files, for a caller that already knows them (and for tests).
	// Ignored when empty, in which case MetricsDir is scanned.
	MetricsFiles []string `name:"metrics-file" help:"Explicit metrics file to enrich. May be repeated." type:"path"`

	// Whether the job had already failed before this ran. Legacy asserted
	// that expected metrics were present, but only when nothing else had
	// failed - otherwise missing metrics are a symptom, not the cause, and
	// failing again just obscures the real error.
	JobStatus string `name:"job-status" env:"AGENT_JOBSTATUS" default:"Succeeded" help:"Job status so far; the presence assertion is skipped unless this is Succeeded."`

	// Written with one path per successfully enriched file. The caller
	// uploads exactly these, so an empty or unreadable file is never
	// ingested and the caller needs no rules of its own about which files
	// are worth uploading.
	EnrichedList string `name:"enriched-list" help:"Write the list of successfully enriched files to this path."`

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

func (h *ProcessMetricsScript) Run() error {
	hostConfigPath := filepath.Join(h.ArtifactDir, scenario.EffectiveHostConfigArtifact)
	if _, err := os.Stat(hostConfigPath); os.IsNotExist(err) {
		// prepare-hc publishes the Host Configuration as its last action, so
		// its absence means the run never deployed anything.
		stormutils.SetAzureDevopsVariables(uploadMetricsVariable, "False")
		logrus.Infof("No Host Configuration at %q; the run produced no metrics to upload.", hostConfigPath)
		return nil
	}

	config, err := metrics.LoadHostConfig(hostConfigPath)
	if err != nil {
		return err
	}

	enrichment := metrics.NewEnrichment(metrics.PipelineContext{
		PipelineName:      h.PipelineName,
		PipelineBuildId:   h.PipelineBuildId,
		PipelineAgentSku:  h.PipelineAgentSku,
		Environment:       h.Environment,
		Location:          h.Location,
		ServerName:        h.ServerName,
		Branch:            h.Branch,
		MachineType:       h.MachineType,
		RuntimeEnv:        h.RuntimeEnv,
		TridentConfigName: h.TridentConfigName,
		TridentCommitHash: h.TridentCommitHash,
		TestRunner:        h.TestRunner,
	}, &config)

	metricsFiles, err := h.resolveMetricsFiles()
	if err != nil {
		return err
	}

	var missing []string
	var failed []string
	var enriched []string
	for _, path := range metricsFiles {
		empty, err := isEmptyOrMissing(path)
		if err != nil {
			logrus.Warnf("Skipping '%s': %v", path, err)
			failed = append(failed, path)
			continue
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
			// Keep going: one torn file (the killed-mid-run case this helper
			// exists to serve) must not cost the run every other file's
			// telemetry. Failures are reported together at the end.
			logrus.Warnf("Failed to enrich '%s': %v", path, err)
			failed = append(failed, path)
			continue
		}
		logrus.Infof("Enriched %d metric record(s) in '%s'", count, path)
		enriched = append(enriched, path)
	}

	// Record what is uploadable BEFORE returning any error: the caller must
	// still be told to upload whatever did enrich.
	if err := h.writeEnrichedList(enriched); err != nil {
		stormutils.SetAzureDevopsVariables(uploadMetricsVariable, "False")
		return err
	}
	stormutils.SetAzureDevopsVariables(uploadMetricsVariable, boolVariable(len(enriched) > 0))

	if len(failed) > 0 {
		return fmt.Errorf("failed to enrich these metrics files: %v", failed)
	}

	if h.jobAlreadyFailed() {
		logrus.Infof("Job status is %q; not asserting that expected metrics are present.", h.JobStatus)
		return nil
	}
	if len(missing) > 0 {
		return fmt.Errorf("expected metrics but these files are missing or empty: %v", missing)
	}

	return nil
}

func (h *ProcessMetricsScript) writeEnrichedList(enriched []string) error {
	if h.EnrichedList == "" {
		return nil
	}

	var content string
	for _, path := range enriched {
		content += path + "\n"
	}
	if err := os.WriteFile(h.EnrichedList, []byte(content), 0644); err != nil {
		return fmt.Errorf("failed to write enriched file list '%s': %w", h.EnrichedList, err)
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

// uploadMetricsVariable is the Azure DevOps variable the caller gates its
// upload steps on.
const uploadMetricsVariable = "UploadMetrics"

// cleanInstallMetricsFile is the trace-stream file the clean install always
// produces; the per-operation files vary by configuration and are discovered.
const cleanInstallMetricsFile = "trident-clean-install-metrics.jsonl"

// servicingMetricsGlob matches the per-operation trace-stream files the
// scenario writes (metrics-<case>.jsonl).
const servicingMetricsGlob = "metrics-*.jsonl"

func boolVariable(v bool) string {
	if v {
		return "True"
	}
	return "False"
}

// jobAlreadyFailed reports whether something had already failed before this
// helper ran, in which case absent metrics are a symptom rather than a cause.
func (h *ProcessMetricsScript) jobAlreadyFailed() bool {
	return !strings.EqualFold(h.JobStatus, "Succeeded")
}

// resolveMetricsFiles returns the metrics files to enrich. Explicit files win;
// otherwise the metrics directory is scanned.
//
// The clean install file is always included even when absent, so that a run
// which should have reported and did not is caught by the presence assertion
// rather than silently skipped. The per-operation files are only expected for
// the configurations that run those phases, so only existing ones are added -
// but empty ones are kept, since an operation that ran without reporting is
// exactly what the assertion exists to catch.
func (h *ProcessMetricsScript) resolveMetricsFiles() ([]string, error) {
	if len(h.MetricsFiles) > 0 {
		return h.MetricsFiles, nil
	}
	if h.MetricsDir == "" {
		return nil, fmt.Errorf("one of --metrics-file or --metrics-dir is required")
	}

	files := []string{filepath.Join(h.MetricsDir, cleanInstallMetricsFile)}

	discovered, err := filepath.Glob(filepath.Join(h.MetricsDir, servicingMetricsGlob))
	if err != nil {
		return nil, fmt.Errorf("failed to scan %q for metrics files: %w", h.MetricsDir, err)
	}
	sort.Strings(discovered)

	return append(files, discovered...), nil
}
